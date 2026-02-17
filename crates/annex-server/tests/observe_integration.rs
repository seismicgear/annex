//! Integration tests verifying that API handlers emit events to the
//! public_event_log table.

use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{api::RegisterResponse, app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt; // for oneshot

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    let vkey_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");
    let vkey_json = std::fs::read_to_string(vkey_path).expect("failed to read vkey");
    let vk = annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");
    Arc::new(vk)
}

fn make_state(pool: annex_db::DbPool) -> AppState {
    let tree = MerkleTree::new(20).unwrap();
    AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: "http://localhost:3000".to_string(),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
    }
}

/// Helper: count events in the public_event_log matching a given domain.
fn count_events_by_domain(pool: &annex_db::DbPool, domain: &str) -> i64 {
    let conn = pool.get().unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM public_event_log WHERE domain = ?1",
        [domain],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

/// Helper: count events in the public_event_log matching a given event_type.
fn count_events_by_type(pool: &annex_db::DbPool, event_type: &str) -> i64 {
    let conn = pool.get().unwrap();
    conn.query_row(
        "SELECT COUNT(*) FROM public_event_log WHERE event_type = ?1",
        [event_type],
        |row| row.get(0),
    )
    .unwrap_or(0)
}

/// Helper: get the payload_json for the latest event of a given type.
fn get_latest_event_payload(pool: &annex_db::DbPool, event_type: &str) -> Option<String> {
    let conn = pool.get().unwrap();
    conn.query_row(
        "SELECT payload_json FROM public_event_log WHERE event_type = ?1 ORDER BY seq DESC LIMIT 1",
        [event_type],
        |row| row.get(0),
    )
    .ok()
}

// ── Registration emits IDENTITY_REGISTERED ──────────────────────────

#[tokio::test]
async fn register_handler_emits_identity_registered_event() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let commitment = "0000000000000000000000000000000000000000000000000000000000000001";
    let body_json = serde_json::json!({
        "commitmentHex": commitment,
        "roleCode": 1,
        "nodeId": 100
    });

    let mut request = Request::builder()
        .uri("/api/registry/register")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body_json.to_string()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = application.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let _resp: RegisterResponse = serde_json::from_slice(&body_bytes).unwrap();

    // Verify IDENTITY_REGISTERED event was persisted
    assert_eq!(count_events_by_type(&pool, "IDENTITY_REGISTERED"), 1);

    // Verify payload structure
    let payload_json = get_latest_event_payload(&pool, "IDENTITY_REGISTERED").unwrap();
    let payload: serde_json::Value = serde_json::from_str(&payload_json).unwrap();
    assert_eq!(payload["event"], "IDENTITY_REGISTERED");
    assert_eq!(payload["commitment_hex"], commitment);
    assert_eq!(payload["role_code"], 1);

    // Verify domain is correct
    assert_eq!(count_events_by_domain(&pool, "IDENTITY"), 1);
}

#[tokio::test]
async fn register_handler_assigns_sequential_seq_numbers() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // Register two identities
    for i in 1..=2u64 {
        let commitment = format!("{:064x}", i);
        let body_json = serde_json::json!({
            "commitmentHex": commitment,
            "roleCode": 1,
            "nodeId": i
        });

        let mut request = Request::builder()
            .uri("/api/registry/register")
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(body_json.to_string()))
            .unwrap();
        request.extensions_mut().insert(ConnectInfo(addr));

        let response = application.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    // Verify two events with sequential seq numbers
    let conn = pool.get().unwrap();
    let seqs: Vec<i64> = {
        let mut stmt = conn
            .prepare("SELECT seq FROM public_event_log ORDER BY seq ASC")
            .unwrap();
        stmt.query_map([], |row| row.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    };

    assert_eq!(seqs, vec![1, 2]);
}

#[tokio::test]
async fn failed_register_does_not_emit_event() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // Use invalid role code → should fail
    let body_json = serde_json::json!({
        "commitmentHex": "0000000000000000000000000000000000000000000000000000000000000001",
        "roleCode": 99,
        "nodeId": 1
    });

    let mut request = Request::builder()
        .uri("/api/registry/register")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(body_json.to_string()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = application.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    // No event should have been emitted
    assert_eq!(count_events_by_type(&pool, "IDENTITY_REGISTERED"), 0);
}
