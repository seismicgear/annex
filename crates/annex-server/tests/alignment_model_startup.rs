//! A production server must not decide who to trust with an instrument its
//! peers do not have.
//!
//! `compare_peer_anchor_scored` produces the score that separates Aligned from
//! Partial from Conflict for every agent registration and every federation
//! handshake. It used to construct a `ConceptEmbedder` inline — a curated
//! twelve-concept lexicon — so the scorer was not a deployment choice, and
//! `StaticEmbedder` over the pinned `potion-base-2M` table was reachable from
//! nothing at all.
//!
//! Now it is a choice, and the two scorers are genuinely different instruments:
//! measured on the same sixteen labelled pairs, the model's unrelated maximum is
//! 0.5134 and the lexicon's 0.3060, with separating bands that do not overlap.
//! A server quietly falling back would reach verdicts a peer running the pinned
//! model cannot reproduce, and the only evidence would be `lexicon-v1` in a
//! handshake nobody reads.
//!
//! So a profile that takes the multi-tenant gates refuses to start without it —
//! the same treatment as the dummy vkey, for the same reason.

use annex_server::{config, startup::prepare_server, startup::StartupError};
use std::sync::OnceLock;
use tokio::sync::Mutex;

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn clear_env() {
    for k in [
        "ANNEX_ZK_KEY_PATH",
        "ANNEX_ZK_KEY_PATH_V2",
        "ANNEX_SIGNING_KEY",
        "ANNEX_UPLOAD_DIR",
        "ANNEX_BUILD_PROFILE",
        "ANNEX_CORS_ORIGINS",
        "ANNEX_EMBEDDING_MODEL_DIR",
    ] {
        std::env::remove_var(k);
    }
}

fn config_for_test() -> config::Config {
    let mut cfg = config::Config::default();
    cfg.database.path = ":memory:".to_string();
    cfg.server.host = std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1));
    cfg.server.port = 0;
    // Not the subject of this file; a missing vkey would fail first and mask it.
    cfg.security.enforce_zk_proofs = false;
    cfg
}

#[tokio::test]
async fn a_production_profile_refuses_to_start_without_the_alignment_model() {
    let _guard = env_lock().lock().await;
    clear_env();
    std::env::set_var("ANNEX_SIGNING_KEY", "05".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );
    std::env::set_var("ANNEX_CORS_ORIGINS", "https://app.example.com");
    std::env::set_var("ANNEX_BUILD_PROFILE", "production");

    // An empty directory: present, readable, and containing no model. This is
    // the realistic shape — a volume that was never populated, not a path
    // typo — and it is the one a `.exists()` check would wave through.
    let empty = tempfile::tempdir().expect("temp dir");
    std::env::set_var("ANNEX_EMBEDDING_MODEL_DIR", empty.path());

    let result = prepare_server(config_for_test()).await;
    clear_env();

    match result {
        Err(StartupError::UnusableAlignmentModel { path, reason }) => {
            assert!(
                path.contains(empty.path().to_str().unwrap()),
                "the error must name the directory an operator has to fix, got {path}"
            );
            assert!(
                reason.contains("model.safetensors"),
                "the error must name the file that is missing, got: {reason}"
            );
        }
        Err(other) => panic!("expected UnusableAlignmentModel, got {other:?}"),
        Ok(_) => panic!(
            "a production server started with no alignment model — it will score \
             trust decisions with the lexicon fallback and its peers will not be \
             able to reproduce a single verdict it reaches"
        ),
    }
}

/// A tampered model is refused for the same reason a missing one is, and the
/// message says which.
#[tokio::test]
async fn a_production_profile_refuses_a_model_that_fails_its_digest() {
    let _guard = env_lock().lock().await;
    clear_env();

    let real = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/embedding");
    if !real.join("model.safetensors").exists() {
        eprintln!("SKIP: no embedding model present to tamper with");
        return;
    }

    let tmp = tempfile::tempdir().expect("temp dir");
    std::fs::copy(
        real.join("tokenizer.json"),
        tmp.path().join("tokenizer.json"),
    )
    .expect("copy tokenizer");
    let mut weights = std::fs::read(real.join("model.safetensors")).expect("read weights");
    // One byte, deep in the tensor data: the file stays the right length and
    // parses fine, which is exactly the case a size check cannot catch and
    // exactly what a corrupted download or a substituted revision looks like.
    let last = weights.len() - 1;
    weights[last] ^= 0x01;
    std::fs::write(tmp.path().join("model.safetensors"), &weights).expect("write weights");

    std::env::set_var("ANNEX_SIGNING_KEY", "06".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );
    std::env::set_var("ANNEX_CORS_ORIGINS", "https://app.example.com");
    std::env::set_var("ANNEX_BUILD_PROFILE", "production");
    std::env::set_var("ANNEX_EMBEDDING_MODEL_DIR", tmp.path());

    let result = prepare_server(config_for_test()).await;
    clear_env();

    match result {
        Err(StartupError::UnusableAlignmentModel { reason, .. }) => {
            assert!(
                reason.contains("digest mismatch"),
                "the error must say the model is not the pinned revision, not merely \
                 that something went wrong, got: {reason}"
            );
        }
        Err(other) => panic!("expected UnusableAlignmentModel, got {other:?}"),
        Ok(_) => panic!("a production server started with a tampered alignment model"),
    }
}

/// A dev profile falls back and keeps working.
///
/// Asserted because the gate must be about the profile, not about the model
/// being hard to obtain — a version of this that refused everywhere would be
/// routed around rather than obeyed, and the e2e harness has no model.
#[tokio::test]
async fn a_dev_profile_falls_back_to_the_lexicon() {
    let _guard = env_lock().lock().await;
    clear_env();
    std::env::set_var("ANNEX_SIGNING_KEY", "07".repeat(32));
    std::env::set_var(
        "ANNEX_UPLOAD_DIR",
        std::env::temp_dir().to_string_lossy().as_ref(),
    );
    std::env::set_var("ANNEX_BUILD_PROFILE", "dev");
    let empty = tempfile::tempdir().expect("temp dir");
    std::env::set_var("ANNEX_EMBEDDING_MODEL_DIR", empty.path());

    let result = prepare_server(config_for_test()).await;
    clear_env();

    assert!(
        result.is_ok(),
        "a dev server must still start without the model: {:?}",
        result.err()
    );
}

// ── Stored verdicts are only as current as the instrument that produced them ──
//
// `agent_registrations.alignment_status` and
// `federation_agreements.alignment_status` are durable, and until now exactly
// one thing recomputed them: `PUT /api/admin/policy`. So a server that upgraded
// to a different scorer — or, as here, to the floor-normalised scale on which
// the previous default threshold of 0.8 was unreachable — kept admitting and
// refusing on verdicts it would no longer reach, and nothing said so. The rows
// simply went on saying what they had always said.

/// Seed a real database file the way an upgraded server's would look:
/// migrations applied, a server row with principles, and one agent whose
/// stored verdict is `CONFLICT` although its anchor matches ours exactly.
fn seed_stale_agent(db_path: &std::path::Path) -> annex_types::ServerPolicy {
    let conn = rusqlite::Connection::open(db_path).expect("open");
    annex_db::run_migrations(&conn).expect("migrations");

    let policy = annex_types::ServerPolicy {
        principles: vec!["treat every participant as a peer".to_string()],
        prohibited_actions: vec!["impersonating another participant".to_string()],
        ..Default::default()
    };
    let policy_json = serde_json::to_string(&policy).unwrap();

    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'stale', 'Stale', ?1)",
        [&policy_json],
    )
    .unwrap();

    // The agent's anchor is byte-identical to the server's, so any working
    // scorer reaches `Aligned` by anchor hash alone. The stored row says
    // otherwise, which is exactly the staleness being detected.
    let anchor = annex_vrp::ServerPolicyRoot::from_policy(&policy)
        .to_anchor_snapshot()
        .expect("anchor");
    let contract = annex_vrp::VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec!["TEXT".to_string()],
        // NOT `'{}'`: that does not deserialize, and the sweep skips a row
        // whose contract it cannot parse — the test would then pass because
        // nothing happened.
        redacted_topics: vec![],
    };

    conn.execute(
        "INSERT INTO agent_registrations \
         (server_id, pseudonym_id, alignment_status, transfer_scope, \
          capability_contract_json, anchor_snapshot_json, last_handshake_at, active) \
         VALUES (1, 'agent-stale', 'CONFLICT', 'NO_TRANSFER', ?1, ?2, datetime('now'), 1)",
        rusqlite::params![
            serde_json::to_string(&contract).unwrap(),
            serde_json::to_string(&anchor).unwrap(),
        ],
    )
    .unwrap();

    policy
}

fn agent_row(db_path: &std::path::Path) -> (String, i64) {
    let conn = rusqlite::Connection::open(db_path).expect("open");
    conn.query_row(
        "SELECT alignment_status, active FROM agent_registrations WHERE pseudonym_id = 'agent-stale'",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .unwrap()
}

fn scorer_id(db_path: &std::path::Path) -> Option<String> {
    let conn = rusqlite::Connection::open(db_path).expect("open");
    conn.query_row(
        "SELECT alignment_scorer_id FROM servers WHERE id = 1",
        [],
        |r| r.get(0),
    )
    .unwrap()
}

#[tokio::test]
async fn a_changed_scorer_re_scores_the_verdicts_it_invalidated() {
    let _guard = env_lock().lock().await;
    clear_env();
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("annex.db");
    seed_stale_agent(&db_path);

    assert_eq!(
        agent_row(&db_path),
        ("CONFLICT".to_string(), 1),
        "fixture precondition",
    );
    assert_eq!(
        scorer_id(&db_path),
        None,
        "nothing has recorded a scorer yet"
    );

    std::env::set_var("ANNEX_SIGNING_KEY", "0a".repeat(32));
    std::env::set_var("ANNEX_UPLOAD_DIR", dir.path());
    std::env::set_var("ANNEX_BUILD_PROFILE", "dev");
    let empty = tempfile::tempdir().expect("temp dir");
    std::env::set_var("ANNEX_EMBEDDING_MODEL_DIR", empty.path());

    let mut cfg = config_for_test();
    cfg.database.path = db_path.to_string_lossy().into_owned();
    let result = prepare_server(cfg).await;
    clear_env();
    result.expect("server should start");

    let (status, active) = agent_row(&db_path);
    assert_eq!(
        status, "ALIGNED",
        "an agent whose anchor matches ours exactly must not still read CONFLICT after a \
         scorer change",
    );
    assert_eq!(active, 1);

    let recorded = scorer_id(&db_path).expect("the scorer that produced these verdicts");
    assert!(
        recorded.contains('@'),
        "the marker should name a model and a digest, got {recorded:?}",
    );
}

#[tokio::test]
async fn a_second_start_with_the_same_scorer_does_not_re_score() {
    // The sweep is not free and must not run on every boot. Proved by
    // corrupting a verdict AFTER the first start recorded the fingerprint: if
    // the second start swept, it would heal it.
    let _guard = env_lock().lock().await;
    clear_env();
    let dir = tempfile::tempdir().expect("temp dir");
    let db_path = dir.path().join("annex.db");
    seed_stale_agent(&db_path);

    std::env::set_var("ANNEX_SIGNING_KEY", "0b".repeat(32));
    std::env::set_var("ANNEX_UPLOAD_DIR", dir.path());
    std::env::set_var("ANNEX_BUILD_PROFILE", "dev");
    let empty = tempfile::tempdir().expect("temp dir");
    std::env::set_var("ANNEX_EMBEDDING_MODEL_DIR", empty.path());

    let mut cfg = config_for_test();
    cfg.database.path = db_path.to_string_lossy().into_owned();
    prepare_server(cfg.clone()).await.expect("first start");
    assert_eq!(agent_row(&db_path).0, "ALIGNED");

    {
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute(
            "UPDATE agent_registrations SET alignment_status = 'PARTIAL' \
             WHERE pseudonym_id = 'agent-stale'",
            [],
        )
        .unwrap();
    }

    prepare_server(cfg).await.expect("second start");
    clear_env();

    assert_eq!(
        agent_row(&db_path).0,
        "PARTIAL",
        "the second start re-scored although the scorer had not changed",
    );
}
