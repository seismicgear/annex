//! Integration tests for RTX subscription endpoints:
//! - `POST /api/rtx/subscribe`
//! - `DELETE /api/rtx/subscribe`
//! - `GET /api/rtx/subscriptions`

use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    let vkey_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");
    if !vkey_path.exists() {
        panic!("vkey not found at {:?}", vkey_path);
    }
    let vkey_json = std::fs::read_to_string(vkey_path).expect("failed to read vkey");
    annex_identity::zk::parse_verification_key(&vkey_json)
        .map(Arc::new)
        .expect("failed to parse vkey")
}

async fn setup_app() -> (axum::Router, annex_db::DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'test-server', 'Test Server', '{}')",
        [],
    )
    .unwrap();

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let policy = ServerPolicy::default();

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: "http://localhost:3000".to_string(),
        policy: Arc::new(RwLock::new(policy)),
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
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
    };

    (app(state), pool)
}

/// Creates an aligned agent registration in the DB.
fn register_agent(pool: &annex_db::DbPool, pseudonym: &str, transfer_scope: &str) {
    let conn = pool.get().unwrap();

    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
         VALUES (1, ?1, 'AI_AGENT', 1)",
        [pseudonym],
    )
    .unwrap();

    let contract_json = serde_json::json!({
        "required_capabilities": [],
        "offered_capabilities": ["TEXT", "VRP"],
        "redacted_topics": []
    })
    .to_string();

    conn.execute(
        "INSERT INTO agent_registrations (
            server_id, pseudonym_id, alignment_status, transfer_scope,
            capability_contract_json, reputation_score, last_handshake_at
        ) VALUES (1, ?1, 'ALIGNED', ?2, ?3, 1.0, datetime('now'))",
        rusqlite::params![pseudonym, transfer_scope, contract_json],
    )
    .unwrap();
}

fn build_subscribe_request(pseudonym: &str, body: &Value) -> Request<Body> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/subscribe")
        .method("POST")
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", pseudonym)
        .body(Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

fn build_unsubscribe_request(pseudonym: &str) -> Request<Body> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/subscribe")
        .method("DELETE")
        .header("X-Annex-Pseudonym", pseudonym)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

fn build_get_subscriptions_request(pseudonym: &str) -> Request<Body> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/subscriptions")
        .method("GET")
        .header("X-Annex-Pseudonym", pseudonym)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

async fn parse_body(response: axum::http::Response<Body>) -> Value {
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap()
}

// ============================================================================
// POST /api/rtx/subscribe — Success Cases
// ============================================================================

#[tokio::test]
async fn test_subscribe_success_empty_filters() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-sub1", "REFLECTION_SUMMARIES_ONLY");

    let body = serde_json::json!({});
    let req = build_subscribe_request("agent-sub1", &body);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = parse_body(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["subscription"]["subscriber_pseudonym"], "agent-sub1");
    assert_eq!(
        body["subscription"]["domain_filters"],
        serde_json::json!([])
    );
    assert_eq!(body["subscription"]["accept_federated"], false);
    assert!(body["subscription"]["created_at"].is_string());
}

#[tokio::test]
async fn test_subscribe_success_with_domain_filters() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-sub2", "FULL_KNOWLEDGE_BUNDLE");

    let body = serde_json::json!({
        "domain_filters": ["rust", "systems", "compilers"],
        "accept_federated": true
    });
    let req = build_subscribe_request("agent-sub2", &body);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = parse_body(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(
        body["subscription"]["domain_filters"],
        serde_json::json!(["rust", "systems", "compilers"])
    );
    assert_eq!(body["subscription"]["accept_federated"], true);
}

#[tokio::test]
async fn test_subscribe_upsert_updates_existing() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-upsert", "REFLECTION_SUMMARIES_ONLY");

    // First subscription
    let body1 = serde_json::json!({
        "domain_filters": ["rust"],
        "accept_federated": false
    });
    let req1 = build_subscribe_request("agent-upsert", &body1);
    let resp1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    // Update subscription (UPSERT)
    let body2 = serde_json::json!({
        "domain_filters": ["python", "ml"],
        "accept_federated": true
    });
    let req2 = build_subscribe_request("agent-upsert", &body2);
    let resp2 = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);

    let parsed = parse_body(resp2).await;
    assert_eq!(
        parsed["subscription"]["domain_filters"],
        serde_json::json!(["python", "ml"]),
        "domain filters should be updated"
    );
    assert_eq!(
        parsed["subscription"]["accept_federated"], true,
        "accept_federated should be updated"
    );

    // Verify only one subscription row exists
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rtx_subscriptions WHERE subscriber_pseudonym = 'agent-upsert'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "UPSERT should not create duplicate subscriptions");
}

// ============================================================================
// POST /api/rtx/subscribe — Failure Cases
// ============================================================================

#[tokio::test]
async fn test_subscribe_requires_auth() {
    let (app, _pool) = setup_app().await;

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/subscribe")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from("{}"))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_subscribe_rejects_no_transfer_scope() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-noscope", "NO_TRANSFER");

    let body = serde_json::json!({});
    let req = build_subscribe_request("agent-noscope", &body);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "NO_TRANSFER agents should not be able to subscribe"
    );
}

#[tokio::test]
async fn test_subscribe_rejects_unregistered_agent() {
    let (app, pool) = setup_app().await;

    // Only create platform identity (no agent registration)
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
             VALUES (1, 'agent-noreg', 'AI_AGENT', 1)",
            [],
        )
        .unwrap();
    }

    let body = serde_json::json!({});
    let req = build_subscribe_request("agent-noreg", &body);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "agents without registration should be rejected"
    );
}

// ============================================================================
// DELETE /api/rtx/subscribe — Success Cases
// ============================================================================

#[tokio::test]
async fn test_unsubscribe_success() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-unsub", "REFLECTION_SUMMARIES_ONLY");

    // Subscribe first
    let body = serde_json::json!({"domain_filters": ["rust"]});
    let req = build_subscribe_request("agent-unsub", &body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Unsubscribe
    let req = build_unsubscribe_request("agent-unsub");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let parsed = parse_body(resp).await;
    assert_eq!(parsed["ok"], true);
    assert!(
        parsed["subscription"].is_null(),
        "subscription should be null after unsubscribe"
    );

    // Verify DB row is gone
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rtx_subscriptions WHERE subscriber_pseudonym = 'agent-unsub'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "subscription row should be deleted");
}

// ============================================================================
// DELETE /api/rtx/subscribe — Failure Cases
// ============================================================================

#[tokio::test]
async fn test_unsubscribe_returns_not_found_when_no_subscription() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-nosub", "REFLECTION_SUMMARIES_ONLY");

    let req = build_unsubscribe_request("agent-nosub");
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "should return 404 when no subscription exists"
    );
}

#[tokio::test]
async fn test_unsubscribe_requires_auth() {
    let (app, _pool) = setup_app().await;

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/subscribe")
        .method("DELETE")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================================
// GET /api/rtx/subscriptions — Success Cases
// ============================================================================

#[tokio::test]
async fn test_get_subscription_returns_existing() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-get", "FULL_KNOWLEDGE_BUNDLE");

    // Subscribe first
    let body = serde_json::json!({
        "domain_filters": ["cryptography", "zk"],
        "accept_federated": true
    });
    let req = build_subscribe_request("agent-get", &body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Get subscription
    let req = build_get_subscriptions_request("agent-get");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let parsed = parse_body(resp).await;
    assert_eq!(parsed["ok"], true);
    assert_eq!(parsed["subscription"]["subscriber_pseudonym"], "agent-get");
    assert_eq!(
        parsed["subscription"]["domain_filters"],
        serde_json::json!(["cryptography", "zk"])
    );
    assert_eq!(parsed["subscription"]["accept_federated"], true);
}

#[tokio::test]
async fn test_get_subscription_returns_null_when_none() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-empty", "REFLECTION_SUMMARIES_ONLY");

    let req = build_get_subscriptions_request("agent-empty");
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let parsed = parse_body(resp).await;
    assert_eq!(parsed["ok"], true);
    assert!(
        parsed["subscription"].is_null(),
        "subscription should be null when none exists"
    );
}

#[tokio::test]
async fn test_get_subscription_requires_auth() {
    let (app, _pool) = setup_app().await;

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/subscriptions")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ============================================================================
// End-to-end: Subscribe → Publish → Verify delivery count
// ============================================================================

#[tokio::test]
async fn test_subscribe_then_publish_delivers_to_subscriber() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-publisher", "FULL_KNOWLEDGE_BUNDLE");
    register_agent(&pool, "agent-subscriber", "REFLECTION_SUMMARIES_ONLY");

    // Subscribe agent-subscriber to "rust" domain
    let sub_body = serde_json::json!({"domain_filters": ["rust"]});
    let req = build_subscribe_request("agent-subscriber", &sub_body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Publish a bundle with matching domain tag
    let bundle = serde_json::json!({
        "bundle_id": format!("bundle-{}", uuid::Uuid::new_v4()),
        "source_pseudonym": "agent-publisher",
        "source_server": "http://localhost:3000",
        "domain_tags": ["rust", "systems"],
        "summary": "Rust ownership model ensures memory safety.",
        "reasoning_chain": "Borrow checker enforces rules at compile time.",
        "caveats": ["Safe Rust only"],
        "created_at": 1700000000000_u64,
        "signature": "sig123",
        "vrp_handshake_ref": "server1:instance1:agreement1"
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/publish")
        .method("POST")
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", "agent-publisher")
        .body(Body::from(bundle.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = parse_body(resp).await;
    assert_eq!(
        body["delivered_to"], 1,
        "should deliver to the subscribed agent"
    );

    // Verify transfer log has a delivery entry for the subscriber
    let conn = pool.get().unwrap();
    let log_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rtx_transfer_log
             WHERE bundle_id = ?1 AND destination_pseudonym = 'agent-subscriber'",
            [bundle["bundle_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(log_count, 1, "delivery should be logged");
}

#[tokio::test]
async fn test_subscribe_then_unsubscribe_stops_delivery() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-pub2", "FULL_KNOWLEDGE_BUNDLE");
    register_agent(&pool, "agent-sub2e2e", "REFLECTION_SUMMARIES_ONLY");

    // Subscribe
    let sub_body = serde_json::json!({"domain_filters": []});
    let req = build_subscribe_request("agent-sub2e2e", &sub_body);
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Unsubscribe
    let req = build_unsubscribe_request("agent-sub2e2e");
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Publish — should deliver to 0 subscribers
    let bundle = serde_json::json!({
        "bundle_id": format!("bundle-{}", uuid::Uuid::new_v4()),
        "source_pseudonym": "agent-pub2",
        "source_server": "http://localhost:3000",
        "domain_tags": ["rust"],
        "summary": "Testing unsubscribe stops delivery.",
        "caveats": [],
        "created_at": 1700000000000_u64,
        "signature": "sig456",
        "vrp_handshake_ref": "server1:instance1:agreement1"
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/publish")
        .method("POST")
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", "agent-pub2")
        .body(Body::from(bundle.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = parse_body(resp).await;
    assert_eq!(
        body["delivered_to"], 0,
        "should not deliver after unsubscription"
    );
}
