//! Integration tests for `POST /api/rtx/publish`.

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

    // Insert server row
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
        public_url: std::sync::Arc::new(std::sync::RwLock::new("http://localhost:3000".to_string())),
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
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
    };

    (app(state), pool)
}

/// Creates an aligned agent registration in the DB so the agent can publish.
fn register_agent(pool: &annex_db::DbPool, pseudonym: &str, transfer_scope: &str) {
    let conn = pool.get().unwrap();

    // Insert platform identity (required for auth middleware)
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
         VALUES (1, ?1, 'AI_AGENT', 1)",
        [pseudonym],
    )
    .unwrap();

    // Insert agent registration with specified transfer scope
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

/// Creates an agent registration with redacted topics.
fn register_agent_with_redactions(
    pool: &annex_db::DbPool,
    pseudonym: &str,
    transfer_scope: &str,
    redacted_topics: &[&str],
) {
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
        "redacted_topics": redacted_topics
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

fn make_bundle(pseudonym: &str) -> Value {
    serde_json::json!({
        "bundle_id": format!("bundle-{}", uuid::Uuid::new_v4()),
        "source_pseudonym": pseudonym,
        "source_server": "http://localhost:3000",
        "domain_tags": ["rust", "systems"],
        "summary": "Rust's ownership model prevents data races at compile time.",
        "reasoning_chain": "Step 1: ownership rules; Step 2: borrow checker; Step 3: lifetimes.",
        "caveats": ["Applies to safe Rust only"],
        "created_at": 1700000000000_u64,
        "signature": "abcdef1234567890",
        "vrp_handshake_ref": "server1:instance1:agreement1"
    })
}

fn build_publish_request(pseudonym: &str, bundle: &Value) -> Request<Body> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/publish")
        .method("POST")
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", pseudonym)
        .body(Body::from(bundle.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

// ============================================================================
// Success Cases
// ============================================================================

#[tokio::test]
async fn test_publish_bundle_success_reflection_scope() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-pub", "REFLECTION_SUMMARIES_ONLY");

    let bundle = make_bundle("agent-pub");
    let req = build_publish_request("agent-pub", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["ok"], true);
    assert!(body["bundleId"].is_string());
    assert_eq!(body["delivered_to"], 0); // No subscribers yet

    // Verify bundle stored in DB
    let conn = pool.get().unwrap();
    let stored: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rtx_bundles WHERE bundle_id = ?1)",
            [bundle["bundle_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(stored, "bundle should be stored in rtx_bundles");

    // Verify reasoning_chain was stripped (scope is ReflectionSummariesOnly)
    let reasoning: Option<String> = conn
        .query_row(
            "SELECT reasoning_chain FROM rtx_bundles WHERE bundle_id = ?1",
            [bundle["bundle_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        reasoning.is_none(),
        "reasoning_chain should be stripped for ReflectionSummariesOnly scope"
    );

    // Verify transfer log
    let log_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rtx_transfer_log WHERE bundle_id = ?1",
            [bundle["bundle_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(log_count, 1, "publish should be logged in transfer log");
}

#[tokio::test]
async fn test_publish_bundle_success_full_scope() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-full", "FULL_KNOWLEDGE_BUNDLE");

    let bundle = make_bundle("agent-full");
    let req = build_publish_request("agent-full", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Verify reasoning_chain is preserved for FullKnowledgeBundle scope
    let conn = pool.get().unwrap();
    let reasoning: Option<String> = conn
        .query_row(
            "SELECT reasoning_chain FROM rtx_bundles WHERE bundle_id = ?1",
            [bundle["bundle_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        reasoning.is_some(),
        "reasoning_chain should be preserved for FullKnowledgeBundle scope"
    );
}

// ============================================================================
// Authentication & Authorization Failures
// ============================================================================

#[tokio::test]
async fn test_publish_requires_auth() {
    let (app, _pool) = setup_app().await;

    let bundle = make_bundle("agent-noauth");

    // No auth header
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/publish")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(bundle.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_publish_rejects_no_transfer_scope() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-notransfer", "NO_TRANSFER");

    let bundle = make_bundle("agent-notransfer");
    let req = build_publish_request("agent-notransfer", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_publish_rejects_mismatched_pseudonym() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-real", "REFLECTION_SUMMARIES_ONLY");

    // Bundle claims source_pseudonym is "agent-fake" but auth is "agent-real"
    let mut bundle = make_bundle("agent-fake");
    // Keep the pseudonym as "agent-fake" in the bundle
    bundle["source_pseudonym"] = serde_json::json!("agent-fake");

    let req = build_publish_request("agent-real", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_publish_rejects_wrong_source_server() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-srv", "REFLECTION_SUMMARIES_ONLY");

    let mut bundle = make_bundle("agent-srv");
    bundle["source_server"] = serde_json::json!("http://evil-server:9999");

    let req = build_publish_request("agent-srv", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_publish_rejects_unregistered_agent() {
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
    } // conn dropped here — must not hold it during oneshot

    let bundle = make_bundle("agent-noreg");
    let req = build_publish_request("agent-noreg", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ============================================================================
// Validation Failures
// ============================================================================

#[tokio::test]
async fn test_publish_rejects_empty_summary() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-val", "REFLECTION_SUMMARIES_ONLY");

    let mut bundle = make_bundle("agent-val");
    bundle["summary"] = serde_json::json!("");

    let req = build_publish_request("agent-val", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_publish_rejects_duplicate_bundle_id() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-dup", "REFLECTION_SUMMARIES_ONLY");

    let bundle = make_bundle("agent-dup");

    // First publish
    let req1 = build_publish_request("agent-dup", &bundle);
    let resp1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    // Second publish with same bundle_id
    let req2 = build_publish_request("agent-dup", &bundle);
    let resp2 = app.oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::CONFLICT);
}

// ============================================================================
// Redacted Topics Enforcement
// ============================================================================

#[tokio::test]
async fn test_publish_enforces_redacted_topics() {
    let (app, pool) = setup_app().await;
    register_agent_with_redactions(
        &pool,
        "agent-redact",
        "REFLECTION_SUMMARIES_ONLY",
        &["politics"],
    );

    // Bundle has domain_tag "politics" which is redacted
    let mut bundle = make_bundle("agent-redact");
    bundle["domain_tags"] = serde_json::json!(["politics", "ethics"]);

    let req = build_publish_request("agent-redact", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "should reject bundles with redacted domain tags"
    );
}

#[tokio::test]
async fn test_publish_allows_non_redacted_topics() {
    let (app, pool) = setup_app().await;
    register_agent_with_redactions(
        &pool,
        "agent-ok",
        "REFLECTION_SUMMARIES_ONLY",
        &["politics"],
    );

    // Bundle has "rust" and "systems" which are not redacted
    let bundle = make_bundle("agent-ok");

    let req = build_publish_request("agent-ok", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "should allow bundles without redacted domain tags"
    );
}

// ============================================================================
// Subscriber Delivery
// ============================================================================

#[tokio::test]
async fn test_publish_delivers_to_subscribers() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-sender", "FULL_KNOWLEDGE_BUNDLE");
    register_agent(&pool, "agent-receiver", "REFLECTION_SUMMARIES_ONLY");

    // Create a subscription for agent-receiver
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO rtx_subscriptions (server_id, subscriber_pseudonym, domain_filters_json)
         VALUES (1, 'agent-receiver', '[]')",
        [],
    )
    .unwrap();
    drop(conn);

    let bundle = make_bundle("agent-sender");
    let req = build_publish_request("agent-sender", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["delivered_to"], 1, "should deliver to one subscriber");

    // Verify delivery was logged in transfer log
    let conn = pool.get().unwrap();
    let log_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rtx_transfer_log WHERE bundle_id = ?1 AND destination_pseudonym = 'agent-receiver'",
            [bundle["bundle_id"].as_str().unwrap()],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(log_count, 1, "delivery should be logged in transfer log");
}

#[tokio::test]
async fn test_publish_respects_subscriber_domain_filters() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-pub-df", "FULL_KNOWLEDGE_BUNDLE");
    register_agent(&pool, "agent-sub-df", "REFLECTION_SUMMARIES_ONLY");

    // Subscribe with domain filter that doesn't match
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO rtx_subscriptions (server_id, subscriber_pseudonym, domain_filters_json)
         VALUES (1, 'agent-sub-df', '[\"python\",\"ml\"]')",
        [],
    )
    .unwrap();
    drop(conn);

    // Bundle has domain_tags ["rust", "systems"] - doesn't match subscriber filter
    let bundle = make_bundle("agent-pub-df");
    let req = build_publish_request("agent-pub-df", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(
        body["delivered_to"], 0,
        "should not deliver to subscriber whose filters don't match"
    );
}

// ============================================================================
// Corrupted Data Safety Tests
// ============================================================================

/// When a subscriber has corrupted `domain_filters_json` in the DB, the system
/// must skip delivery to that subscriber (reject) rather than defaulting to
/// accept-all, which could cause unauthorized knowledge transfer.
#[tokio::test]
async fn test_publish_skips_subscriber_with_corrupted_domain_filters() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-pub-corrupt", "FULL_KNOWLEDGE_BUNDLE");
    register_agent(&pool, "agent-sub-corrupt", "REFLECTION_SUMMARIES_ONLY");
    register_agent(&pool, "agent-sub-good", "REFLECTION_SUMMARIES_ONLY");

    // Insert one subscription with corrupted JSON and one with valid JSON
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO rtx_subscriptions (server_id, subscriber_pseudonym, domain_filters_json)
         VALUES (1, 'agent-sub-corrupt', 'NOT_VALID_JSON{{{}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO rtx_subscriptions (server_id, subscriber_pseudonym, domain_filters_json)
         VALUES (1, 'agent-sub-good', '[]')",
        [],
    )
    .unwrap();
    drop(conn);

    let bundle = make_bundle("agent-pub-corrupt");
    let req = build_publish_request("agent-pub-corrupt", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    // Only the good subscriber should receive delivery; the corrupted one is skipped
    assert_eq!(
        body["delivered_to"], 1,
        "corrupted domain_filters_json must cause skip (reject), not accept-all"
    );
}

/// When a subscriber has a transfer scope that cannot be enforced, the system
/// must skip that subscriber and log the failure rather than silently dropping it.
#[tokio::test]
async fn test_publish_skips_subscriber_with_unparseable_scope() {
    let (app, pool) = setup_app().await;
    register_agent(&pool, "agent-pub-scope", "FULL_KNOWLEDGE_BUNDLE");

    // Create a subscriber identity + registration with a corrupted transfer_scope
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
         VALUES (1, 'agent-sub-badscope', 'AI_AGENT', 1)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO agent_registrations (
            server_id, pseudonym_id, alignment_status, transfer_scope,
            capability_contract_json, reputation_score, last_handshake_at
        ) VALUES (1, 'agent-sub-badscope', 'ALIGNED', 'INVALID_SCOPE_VALUE', '{}', 1.0, datetime('now'))",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO rtx_subscriptions (server_id, subscriber_pseudonym, domain_filters_json)
         VALUES (1, 'agent-sub-badscope', '[]')",
        [],
    )
    .unwrap();
    drop(conn);

    let bundle = make_bundle("agent-pub-scope");
    let req = build_publish_request("agent-pub-scope", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    // Invalid scope → subscriber is skipped, not crashed
    assert_eq!(
        body["delivered_to"], 0,
        "subscriber with unparseable transfer scope must be skipped"
    );
}
