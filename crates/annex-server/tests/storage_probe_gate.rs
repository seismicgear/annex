//! The proactive half of the storage gate, end to end.
//!
//! `storage_health`'s module header documented two ways the gate is
//! promoted to `Degraded`: a reactive `SQLITE_FULL` trip, and a
//! background probe comparing the database file against the operator's
//! thresholds. Only the first was wired. `evaluate_db_file_size` had no
//! caller outside its own unit tests, no `max_db_bytes` existed for its
//! thresholds to be headroom against, and `ANNEX_STORAGE_BLOCK_FREE_BYTES`
//! — documented as the point at which "the server refuses writes with
//! HTTP 507" — changed nothing an operator could observe.
//!
//! That is invisible to a unit test, because every unit was correct.
//! These tests run the real task against a real database file and assert
//! on HTTP status codes, which is the only place the defect showed.

mod common;

use annex_db::{create_pool, DbPool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::config::StorageConfig;
use annex_server::state::AppState;
use annex_server::storage_health::{StorageHealth, StorageState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

/// A real on-disk database — the probe calls `std::fs::metadata`, so an
/// in-memory pool would give it nothing to measure.
struct Fixture {
    app: axum::Router,
    state: Arc<AppState>,
    health: Arc<StorageHealth>,
    db_path: PathBuf,
    _dir: tempfile::TempDir,
}

/// `thresholds` is `(headroom, block, warn)`, all relative to the
/// database that exists once the migrations have run — so the tests do
/// not depend on how many bytes those migrations happen to write.
/// `None` leaves the shipped defaults, which are uncapped.
fn setup(thresholds: Option<(u64, u64, u64)>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("annex-probe-test.db");
    let pool: DbPool = create_pool(
        db_path.to_str().expect("temp path is utf-8"),
        DbRuntimeSettings::default(),
    )
    .unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        let policy_json = serde_json::to_string(&ServerPolicy::default()).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active)
             VALUES (1, 'mod_user', 'HUMAN', 1, 1)",
            [],
        )
        .unwrap();
    }

    let storage = match thresholds {
        None => StorageConfig::default(),
        Some((headroom, block, warn)) => {
            // Main file plus WAL sidecars — the same set the probe measures.
            // Sizing against the main file alone made the warn-threshold test
            // land in `Degraded` instead: the pool opens in WAL mode and the
            // `-wal` file is well over a kilobyte after the migrations, so the
            // headroom the test thought it was leaving had already been spent.
            let size: u64 = ["", "-wal", "-shm"]
                .iter()
                .filter_map(|suffix| {
                    let mut name = db_path.clone().into_os_string();
                    name.push(suffix);
                    std::fs::metadata(std::path::PathBuf::from(name)).ok()
                })
                .map(|m| m.len())
                .sum();
            StorageConfig {
                max_db_bytes: size + headroom,
                block_free_bytes: block,
                warn_free_bytes: warn,
                ..StorageConfig::default()
            }
        }
    };

    let tree = MerkleTree::new(20).unwrap();
    let mut state = common::build_app_state(pool, tree, ServerPolicy::default());
    state.storage_config = storage;
    let health = state.storage_health.clone();
    let app = annex_server::app(state.clone());
    Fixture {
        app,
        state: Arc::new(state),
        health,
        db_path,
        _dir: dir,
    }
}

fn req(method: &str, uri: &str, body: Body) -> Request<Body> {
    let mut r = Request::builder()
        .uri(uri)
        .method(method)
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", "mod_user")
        .body(body)
        .unwrap();
    r.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    r
}

/// The probe's first pass runs before its first sleep, but it hops
/// through `spawn_blocking`, so poll rather than assume it has landed.
async fn await_state(health: &StorageHealth, want: StorageState) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if health.state() == want {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "storage gate never reached {:?} (still {:?}: {})",
        want,
        health.state(),
        health.reason()
    );
}

/// The defect, stated as a test: an operator sets a cap and a blocking
/// threshold, the database is inside it, and writes must stop.
#[tokio::test]
async fn probe_closes_the_gate_and_writes_get_507() {
    let fx = setup(Some((1_000, 2_000, 4_000)));

    assert!(
        !fx.health.writes_blocked(),
        "gate must start open — the probe is what closes it"
    );

    tokio::spawn(annex_server::background::start_storage_probe_task(
        fx.state.clone(),
        fx.db_path.clone(),
    ));
    await_state(&fx.health, StorageState::Degraded).await;

    let response = fx
        .app
        .clone()
        .oneshot(req(
            "PATCH",
            "/api/admin/server",
            Body::from(r#"{"label":"New Label"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(
        response.status(),
        StatusCode::INSUFFICIENT_STORAGE,
        "the probe closed the gate, so writes must 507"
    );

    // Reads are deliberately unaffected: a full disk must not take the
    // server offline for everyone already on it.
    let response = fx
        .app
        .oneshot(req("GET", "/api/admin/storage", Body::empty()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK, "reads must keep flowing");
}

/// The warning threshold has to be a distinct, non-blocking state, or
/// there is no early signal at all — just a server that works and then
/// abruptly does not.
#[tokio::test]
async fn probe_at_warn_threshold_leaves_writes_flowing() {
    let fx = setup(Some((3_000, 2_000, 4_000)));

    tokio::spawn(annex_server::background::start_storage_probe_task(
        fx.state.clone(),
        fx.db_path.clone(),
    ));
    await_state(&fx.health, StorageState::Warn).await;

    assert!(
        !fx.health.writes_blocked(),
        "warn is an operational signal, not a gate"
    );
    let response = fx
        .app
        .oneshot(req(
            "PATCH",
            "/api/admin/server",
            Body::from(r#"{"label":"New Label"}"#),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

/// Uncapped is the default, and it must stay inert rather than guessing
/// a cap. The task returns instead of looping, so an operator who never
/// sets one pays nothing.
#[tokio::test]
async fn probe_without_a_cap_returns_instead_of_looping() {
    let fx = setup(None);
    assert_eq!(
        fx.state.storage_config.max_db_bytes, 0,
        "uncapped must be the default"
    );

    tokio::time::timeout(
        Duration::from_secs(5),
        annex_server::background::start_storage_probe_task(fx.state.clone(), fx.db_path.clone()),
    )
    .await
    .expect("the task must return immediately when no cap is configured");

    assert_eq!(fx.health.state(), StorageState::Healthy);
}

/// A gate an operator has cleared must not be re-closed by a probe that
/// merely measured a file it had already accounted for — but one that is
/// genuinely still over the cap must close again. This pins the second
/// half: the probe is not one-shot.
#[tokio::test]
async fn probe_recloses_the_gate_after_an_operator_clears_it_prematurely() {
    let fx = setup(Some((1_000, 2_000, 4_000)));

    // One pass, by hand — the same call the task makes.
    annex_server::storage_health::evaluate_db_file_size(
        &fx.health,
        &fx.db_path,
        fx.state.storage_config.warn_free_bytes,
        fx.state.storage_config.block_free_bytes,
        Some(fx.state.storage_config.max_db_bytes),
    );
    assert!(fx.health.writes_blocked());

    let response = fx
        .app
        .clone()
        .oneshot(req("POST", "/api/admin/storage/clear", Body::empty()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!fx.health.writes_blocked(), "operator cleared the gate");

    // Nothing was freed, so the next pass must close it again rather
    // than leaving the operator with a gate that stays open on a full
    // disk because they clicked the button once.
    annex_server::storage_health::evaluate_db_file_size(
        &fx.health,
        &fx.db_path,
        fx.state.storage_config.warn_free_bytes,
        fx.state.storage_config.block_free_bytes,
        Some(fx.state.storage_config.max_db_bytes),
    );
    assert!(
        fx.health.writes_blocked(),
        "the probe must re-close a gate whose cause has not gone away"
    );
}
