//! Integration tests verifying that API handlers emit events to the
//! public_event_log table and that the public event API endpoints work.

mod common;

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

fn make_state(pool: annex_db::DbPool) -> AppState {
    let tree = MerkleTree::new(20).unwrap();
    AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: common::load_vkey_or_dummy(),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: std::sync::Arc::new(std::sync::RwLock::new(
            "http://localhost:3000".to_string(),
        )),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::WebRtcConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        invite_base_url: "https://monolithannex.com/invite".to_string(),
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
        voice_token_secret: std::sync::Arc::new([0u8; 32]),
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
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
async fn handler_emitted_events_carry_verifiable_signatures() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    let state = make_state(pool.clone());
    // Clone the Arc so we can verify against the same key the handlers
    // sign with (ADR-0013).
    let signing_key = state.signing_key.clone();
    let application = app(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
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
    let response = application.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let conn = pool.get().unwrap();

    // Every emitted row carries a signature (no NULLs on the live path).
    let unsigned: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM public_event_log WHERE event_signature IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        unsigned, 0,
        "handler-emitted events must be signed (ADR-0013)"
    );

    // And the signatures verify against the server's key.
    assert!(
        annex_observe::verify_event_log_signatures(&conn, 1, &signing_key.verifying_key())
            .expect("verify")
            .is_none(),
        "handler-emitted event signatures must verify against the server key"
    );
    assert!(
        annex_observe::verify_event_log_chain(&conn, 1)
            .expect("verify")
            .is_none(),
        "hash chain must remain intact"
    );
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
        let commitment = format!("{i:064x}");
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

// ── GET /api/public/events/chain ────────────────────────────────────

/// Plays the role of an external auditor: fetch the chain export and
/// verify the hash chain AND the per-event signatures using only the
/// response data (rows + verifying key + signing domain) — no access
/// to server internals.
#[tokio::test]
async fn chain_export_supports_full_offline_verification() {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // Create three events through the real handler path.
    for i in 1..=3u64 {
        let body_json = serde_json::json!({
            "commitmentHex": format!("{i:064x}"),
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

    let mut request = Request::builder()
        .uri("/api/public/events/chain")
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

    assert_eq!(resp["count"], 3);
    assert_eq!(resp["signing_domain"], "annex-event-v1");
    assert!(resp["next_from_seq"].is_null());

    // Offline verification using only response data.
    let key_bytes: [u8; 32] = hex::decode(resp["server_verifying_key"].as_str().unwrap())
        .unwrap()
        .try_into()
        .unwrap();
    let verifying_key = VerifyingKey::from_bytes(&key_bytes).unwrap();
    let signing_domain = resp["signing_domain"].as_str().unwrap();

    let mut expected_prev = "GENESIS".to_string();
    for row in resp["rows"].as_array().unwrap() {
        // 1. Chain linkage + canonical hash.
        assert_eq!(row["prev_hash"].as_str().unwrap(), expected_prev);
        let recomputed = annex_observe::compute_event_hash(
            1,
            row["seq"].as_i64().unwrap(),
            row["domain"].as_str().unwrap(),
            row["event_type"].as_str().unwrap(),
            row["entity_type"].as_str().unwrap(),
            row["entity_id"].as_str().unwrap(),
            row["payload_json"].as_str().unwrap(),
            row["occurred_at"].as_str().unwrap(),
            row["prev_hash"].as_str().unwrap(),
        );
        assert_eq!(row["event_hash"].as_str().unwrap(), recomputed);

        // 2. Signature over the documented signing input.
        let sig_bytes: [u8; 64] = hex::decode(row["event_signature"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let signature = Signature::from_bytes(&sig_bytes);
        verifying_key
            .verify(
                format!("{signing_domain}\n{recomputed}").as_bytes(),
                &signature,
            )
            .expect("exported signature must verify under the exported key");

        expected_prev = recomputed;
    }

    // Pagination: limit=2 returns 2 rows + next_from_seq=3.
    let mut request = Request::builder()
        .uri("/api/public/events/chain?limit=2")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));
    let response = application.clone().oneshot(request).await.unwrap();
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(resp["count"], 2);
    assert_eq!(resp["next_from_seq"], 3);

    // Second page picks up where the first left off.
    let mut request = Request::builder()
        .uri("/api/public/events/chain?from_seq=3&limit=2")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));
    let response = application.oneshot(request).await.unwrap();
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: serde_json::Value = serde_json::from_slice(&body_bytes).unwrap();
    assert_eq!(resp["count"], 1);
    assert_eq!(resp["rows"][0]["seq"], 3);
    assert!(resp["next_from_seq"].is_null());
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
        "expected text/event-stream, got: {content_type}"
    );
}

// ── GET /api/public/server/summary ──────────────────────────────────

#[tokio::test]
async fn server_summary_returns_aggregate_counts() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();

        // Seed server
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test Server', '{}')",
            [],
        )
        .unwrap();

        // Seed graph nodes
        conn.execute(
            "INSERT INTO graph_nodes (server_id, pseudonym_id, node_type, active) VALUES (1, 'p1', 'Human', 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO graph_nodes (server_id, pseudonym_id, node_type, active) VALUES (1, 'p2', 'Human', 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO graph_nodes (server_id, pseudonym_id, node_type, active) VALUES (1, 'a1', 'AiAgent', 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO graph_nodes (server_id, pseudonym_id, node_type, active) VALUES (1, 'a2', 'AiAgent', 0)",
            [],
        ).unwrap();

        // Seed channels
        conn.execute(
            "INSERT INTO channels (server_id, channel_id, name, channel_type) VALUES (1, 'ch1', 'General', 'TEXT')",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO channels (server_id, channel_id, name, channel_type) VALUES (1, 'ch2', 'Voice', 'VOICE')",
            [],
        ).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/public/server/summary")
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

    // 2 Human + 1 active AiAgent = 3 total active
    assert_eq!(resp["total_active_members"], 3);
    assert_eq!(resp["members_by_type"]["Human"], 2);
    assert_eq!(resp["members_by_type"]["AiAgent"], 1);
    assert_eq!(resp["channel_count"], 2);
    // No federation agreements seeded
    assert_eq!(resp["federation_peer_count"], 0);
    assert!(!resp["slug"].as_str().unwrap().is_empty());
    assert!(!resp["label"].as_str().unwrap().is_empty());
}

#[tokio::test]
async fn server_summary_empty_server() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test Server', '{}')",
            [],
        )
        .unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/public/server/summary")
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

    assert_eq!(resp["total_active_members"], 0);
    assert_eq!(resp["channel_count"], 0);
    assert_eq!(resp["federation_peer_count"], 0);
    assert_eq!(resp["active_agent_count"], 0);
}

// ── GET /api/public/federation/peers ────────────────────────────────

#[tokio::test]
async fn federation_peers_returns_active_agreements() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();

        // Seed remote instances
        conn.execute(
            "INSERT INTO instances (base_url, public_key, label, status) VALUES ('https://alpha.example.com', 'key1', 'Alpha Node', 'ACTIVE')",
            [],
        ).unwrap();
        let instance1_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO instances (base_url, public_key, label, status) VALUES ('https://beta.example.com', 'key2', 'Beta Node', 'ACTIVE')",
            [],
        ).unwrap();
        let instance2_id = conn.last_insert_rowid();

        // Seed federation agreements
        conn.execute(
            "INSERT INTO federation_agreements (local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active)
             VALUES (1, ?1, 'Aligned', 'FULL_KNOWLEDGE_BUNDLE', '{}', 1)",
            rusqlite::params![instance1_id],
        ).unwrap();
        conn.execute(
            "INSERT INTO federation_agreements (local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active)
             VALUES (1, ?1, 'Partial', 'REFLECTION_SUMMARIES_ONLY', '{}', 1)",
            rusqlite::params![instance2_id],
        ).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/public/federation/peers")
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
    let peers = resp["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 2);

    // Ordered by label ASC: Alpha, Beta
    assert_eq!(peers[0]["label"], "Alpha Node");
    assert_eq!(peers[0]["alignment_status"], "Aligned");
    assert_eq!(peers[0]["transfer_scope"], "FULL_KNOWLEDGE_BUNDLE");
    assert_eq!(peers[0]["active"], true);
    assert_eq!(peers[1]["label"], "Beta Node");
    assert_eq!(peers[1]["alignment_status"], "Partial");

    // Every row carries the agreement id.
    //
    // Without it the list is not addressable. The client's `FederationPeer`
    // declared an `instance_id` this endpoint has never sent, so
    // `FederationPanel` keyed every row on `undefined`; and
    // `DELETE /api/admin/federation/{id}` takes an agreement id no endpoint
    // returned, so revoking one was unreachable from any client.
    //
    // `base_url` is not a substitute: there is no unique constraint on
    // (local_server_id, remote_instance_id), so two active rows can name the
    // same instance — which `two_agreements_with_one_instance_stay_distinct`
    // below actually does.
    let ids: Vec<i64> = peers
        .iter()
        .map(|p| {
            p["agreement_id"]
                .as_i64()
                .unwrap_or_else(|| panic!("peer row has no agreement_id: {p}"))
        })
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1], "two agreements shared one id: {ids:?}");
}

#[tokio::test]
async fn two_agreements_with_one_instance_stay_distinct() {
    // The schema permits it — no unique constraint on
    // (local_server_id, remote_instance_id) — so a re-handshake can leave two
    // active rows pointing at one peer. They are two revocable agreements and
    // must not collapse into one row or share a key.
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO instances (base_url, public_key, label, status) VALUES ('https://twice.example.com', 'key1', 'Twice Node', 'ACTIVE')",
            [],
        ).unwrap();
        let instance_id = conn.last_insert_rowid();

        for scope in ["FULL_KNOWLEDGE_BUNDLE", "REFLECTION_SUMMARIES_ONLY"] {
            conn.execute(
                "INSERT INTO federation_agreements (local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active)
                 VALUES (1, ?1, 'Aligned', ?2, '{}', 1)",
                rusqlite::params![instance_id, scope],
            ).unwrap();
        }
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/public/federation/peers")
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

    let peers = resp["peers"].as_array().unwrap();
    assert_eq!(peers.len(), 2, "both agreements should be listed: {resp}");
    assert_eq!(peers[0]["base_url"], peers[1]["base_url"]);
    assert_ne!(
        peers[0]["agreement_id"], peers[1]["agreement_id"],
        "same base_url, so the agreement id is the only thing telling these \
         two rows apart: {resp}",
    );
}

#[tokio::test]
async fn federation_peers_excludes_inactive() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();

        conn.execute(
            "INSERT INTO instances (base_url, public_key, label, status) VALUES ('https://dead.example.com', 'key1', 'Dead Node', 'ACTIVE')",
            [],
        ).unwrap();
        let instance_id = conn.last_insert_rowid();

        conn.execute(
            "INSERT INTO federation_agreements (local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active)
             VALUES (1, ?1, 'Conflict', 'NO_TRANSFER', '{}', 0)",
            rusqlite::params![instance_id],
        ).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/public/federation/peers")
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
}

// ── GET /api/public/agents ──────────────────────────────────────────

#[tokio::test]
async fn agents_returns_active_agents_ordered_by_reputation() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();

        // Seed server (foreign key target)
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test Server', '{}')",
            [],
        )
        .unwrap();

        conn.execute(
            "INSERT INTO agent_registrations (server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, reputation_score, last_handshake_at, active)
             VALUES (1, 'agent-a', 'Aligned', 'FULL_KNOWLEDGE_BUNDLE', '{\"can_moderate\":false}', 0.8, datetime('now'), 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO agent_registrations (server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, reputation_score, last_handshake_at, active)
             VALUES (1, 'agent-b', 'Partial', 'REFLECTION_SUMMARIES_ONLY', '{\"can_moderate\":true}', 0.95, datetime('now'), 1)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO agent_registrations (server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, reputation_score, last_handshake_at, active)
             VALUES (1, 'agent-inactive', 'Aligned', 'FULL_KNOWLEDGE_BUNDLE', '{}', 0.5, datetime('now'), 0)",
            [],
        ).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/public/agents")
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

    assert_eq!(resp["count"], 2); // Only active agents
    let agents = resp["agents"].as_array().unwrap();

    // Ordered by reputation DESC: agent-b (0.95), agent-a (0.8)
    assert_eq!(agents[0]["pseudonym_id"], "agent-b");
    assert_eq!(agents[0]["reputation_score"], 0.95);
    assert_eq!(agents[0]["alignment_status"], "Partial");
    assert_eq!(agents[0]["capability_contract"]["can_moderate"], true);

    assert_eq!(agents[1]["pseudonym_id"], "agent-a");
    assert_eq!(agents[1]["reputation_score"], 0.8);
}

#[tokio::test]
async fn agents_returns_empty_when_no_agents() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    let state = make_state(pool.clone());
    let application = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/public/agents")
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
    assert!(resp["agents"].as_array().unwrap().is_empty());
}
