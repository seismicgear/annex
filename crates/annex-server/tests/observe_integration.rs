//! Integration tests verifying that API handlers emit events to the
//! public_event_log table and that the public event API endpoints work.

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
        observe_tx: tokio::sync::broadcast::channel(256).0,
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

// ── GET /api/public/events ──────────────────────────────────────────

#[tokio::test]
async fn get_events_returns_persisted_events() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // Register an identity to create an event
    let body_json = serde_json::json!({
        "commitmentHex": "0000000000000000000000000000000000000000000000000000000000000001",
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
    let response = application.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Now query the events API
    let mut request = Request::builder()
        .uri("/api/public/events")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = application.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    assert!(resp["count"].as_u64().unwrap() >= 1);
    let events = resp["events"].as_array().unwrap();
    assert!(!events.is_empty());

    // First event should be IDENTITY_REGISTERED
    assert_eq!(events[0]["event_type"], "IDENTITY_REGISTERED");
    assert_eq!(events[0]["domain"], "IDENTITY");
    assert_eq!(events[0]["entity_type"], "identity");
    assert_eq!(events[0]["seq"], 1);
}

#[tokio::test]
async fn get_events_filters_by_domain() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    // Seed events directly into the database
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO public_event_log (server_id, domain, event_type, entity_type, entity_id, seq, payload_json, occurred_at)
             VALUES (1, 'IDENTITY', 'IDENTITY_REGISTERED', 'identity', 'c1', 1, '{}', datetime('now'))",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO public_event_log (server_id, domain, event_type, entity_type, entity_id, seq, payload_json, occurred_at)
             VALUES (1, 'PRESENCE', 'NODE_ADDED', 'node', 'p1', 2, '{}', datetime('now'))",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO public_event_log (server_id, domain, event_type, entity_type, entity_id, seq, payload_json, occurred_at)
             VALUES (1, 'IDENTITY', 'IDENTITY_VERIFIED', 'identity', 'c1', 3, '{}', datetime('now'))",
            [],
        ).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // Filter by IDENTITY domain
    let mut request = Request::builder()
        .uri("/api/public/events?domain=IDENTITY")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = application.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(resp["count"], 2);
    let events = resp["events"].as_array().unwrap();
    assert!(events.iter().all(|e| e["domain"] == "IDENTITY"));

    // Filter by PRESENCE domain
    let mut request = Request::builder()
        .uri("/api/public/events?domain=PRESENCE")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = application.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(resp["count"], 1);
}

#[tokio::test]
async fn get_events_respects_limit() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    // Seed 5 events
    {
        let conn = pool.get().unwrap();
        for i in 1..=5 {
            conn.execute(
                "INSERT INTO public_event_log (server_id, domain, event_type, entity_type, entity_id, seq, payload_json, occurred_at)
                 VALUES (1, 'IDENTITY', 'IDENTITY_REGISTERED', 'identity', ?1, ?2, '{}', datetime('now'))",
                rusqlite::params![format!("c{i}"), i],
            ).unwrap();
        }
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/public/events?limit=2")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = application.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(resp["count"], 2);
}

#[tokio::test]
async fn get_events_rejects_invalid_domain() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/public/events?domain=INVALID")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = application.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_events_returns_empty_when_no_events() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/public/events")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = application.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(resp["count"], 0);
    assert!(resp["events"].as_array().unwrap().is_empty());
}

// ── GET /events/stream (SSE) ────────────────────────────────────────

#[tokio::test]
async fn event_stream_returns_sse_content_type() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/events/stream")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = application.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let content_type = response
        .headers()
        .get("content-type")
        .expect("should have content-type header")
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("text/event-stream"),
        "expected text/event-stream, got: {}",
        content_type
    );
}
