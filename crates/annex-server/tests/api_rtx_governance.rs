//! Integration tests for RTX governance mediation endpoints (Step 9.5).
//!
//! Tests:
//! - `GET /api/rtx/governance/transfers` — paginated transfer log query
//! - `GET /api/rtx/governance/summary` — aggregate transfer statistics
//! - Permission enforcement (requires `can_moderate`)
//! - Filtering by bundle_id, source, destination, since, until
//! - Pagination (limit, offset)

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

fn load_dummy_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
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
        membership_vkey: load_dummy_vkey(),
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
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
    };

    (app(state), pool)
}

/// Creates a platform identity with `can_moderate = true` (operator).
fn register_operator(pool: &annex_db::DbPool, pseudonym: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active)
         VALUES (1, ?1, 'HUMAN', 1, 1)",
        [pseudonym],
    )
    .unwrap();
}

/// Creates a platform identity with `can_moderate = false` (regular user).
fn register_regular_user(pool: &annex_db::DbPool, pseudonym: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active)
         VALUES (1, ?1, 'HUMAN', 0, 1)",
        [pseudonym],
    )
    .unwrap();
}

/// Inserts a transfer log entry directly into the database.
fn insert_transfer_log(
    pool: &annex_db::DbPool,
    bundle_id: &str,
    source: &str,
    destination: Option<&str>,
    scope: &str,
    redactions: Option<&str>,
) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO rtx_transfer_log (
            server_id, bundle_id, source_pseudonym, destination_pseudonym,
            transfer_scope_applied, redactions_applied
        ) VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![bundle_id, source, destination, scope, redactions],
    )
    .unwrap();
}

fn build_get_request(pseudonym: &str, uri: &str) -> Request<Body> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri(uri)
        .method("GET")
        .header("X-Annex-Pseudonym", pseudonym)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

// ============================================================================
// Permission Tests
// ============================================================================

#[tokio::test]
async fn test_governance_transfers_requires_auth() {
    let (app, _pool) = setup_app().await;

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/governance/transfers")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_governance_transfers_requires_can_moderate() {
    let (app, pool) = setup_app().await;
    register_regular_user(&pool, "user-nomod");

    let req = build_get_request("user-nomod", "/api/rtx/governance/transfers");
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_governance_summary_requires_auth() {
    let (app, _pool) = setup_app().await;

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/rtx/governance/summary")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_governance_summary_requires_can_moderate() {
    let (app, pool) = setup_app().await;
    register_regular_user(&pool, "user-nomod2");

    let req = build_get_request("user-nomod2", "/api/rtx/governance/summary");
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

// ============================================================================
// Transfers Endpoint — Basic
// ============================================================================

#[tokio::test]
async fn test_governance_transfers_empty() {
    let (app, pool) = setup_app().await;
    register_operator(&pool, "operator");

    let req = build_get_request("operator", "/api/rtx/governance/transfers");
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total"], 0);
    assert_eq!(body["transfers"].as_array().unwrap().len(), 0);
    assert_eq!(body["limit"], 50);
    assert_eq!(body["offset"], 0);
}

#[tokio::test]
async fn test_governance_transfers_returns_entries() {
    let (app, pool) = setup_app().await;
    register_operator(&pool, "operator");

    // Insert some transfer log entries
    insert_transfer_log(
        &pool,
        "bundle-1",
        "agent-alpha",
        None,
        "FULL_KNOWLEDGE_BUNDLE",
        None,
    );
    insert_transfer_log(
        &pool,
        "bundle-1",
        "agent-alpha",
        Some("agent-beta"),
        "REFLECTION_SUMMARIES_ONLY",
        Some("reasoning_chain_stripped"),
    );
    insert_transfer_log(
        &pool,
        "bundle-2",
        "agent-gamma",
        None,
        "REFLECTION_SUMMARIES_ONLY",
        Some("reasoning_chain_stripped"),
    );

    let req = build_get_request("operator", "/api/rtx/governance/transfers");
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total"], 3);
    let transfers = body["transfers"].as_array().unwrap();
    assert_eq!(transfers.len(), 3);

    // Results are ordered by id DESC (most recent first)
    assert_eq!(transfers[0]["bundle_id"], "bundle-2");
    assert_eq!(transfers[1]["bundle_id"], "bundle-1");
    assert_eq!(transfers[1]["destination_pseudonym"], "agent-beta");
    assert_eq!(
        transfers[1]["redactions_applied"],
        "reasoning_chain_stripped"
    );
    assert_eq!(transfers[2]["bundle_id"], "bundle-1");
    assert!(transfers[2]["destination_pseudonym"].is_null());
}

// ============================================================================
// Transfers Endpoint — Filtering
// ============================================================================

#[tokio::test]
async fn test_governance_transfers_filter_by_bundle_id() {
    let (app, pool) = setup_app().await;
    register_operator(&pool, "operator");

    insert_transfer_log(&pool, "b-100", "src-a", None, "FULL_KNOWLEDGE_BUNDLE", None);
    insert_transfer_log(&pool, "b-200", "src-b", None, "FULL_KNOWLEDGE_BUNDLE", None);

    let req = build_get_request("operator", "/api/rtx/governance/transfers?bundle_id=b-100");
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total"], 1);
    assert_eq!(body["transfers"][0]["bundle_id"], "b-100");
}

#[tokio::test]
async fn test_governance_transfers_filter_by_source() {
    let (app, pool) = setup_app().await;
    register_operator(&pool, "operator");

    insert_transfer_log(&pool, "b-1", "agent-x", None, "FULL_KNOWLEDGE_BUNDLE", None);
    insert_transfer_log(&pool, "b-2", "agent-y", None, "FULL_KNOWLEDGE_BUNDLE", None);
    insert_transfer_log(
        &pool,
        "b-3",
        "agent-x",
        Some("agent-z"),
        "REFLECTION_SUMMARIES_ONLY",
        None,
    );

    let req = build_get_request("operator", "/api/rtx/governance/transfers?source=agent-x");
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total"], 2);
}

#[tokio::test]
async fn test_governance_transfers_filter_by_destination() {
    let (app, pool) = setup_app().await;
    register_operator(&pool, "operator");

    insert_transfer_log(&pool, "b-1", "src", None, "FULL_KNOWLEDGE_BUNDLE", None);
    insert_transfer_log(
        &pool,
        "b-1",
        "src",
        Some("dst-a"),
        "REFLECTION_SUMMARIES_ONLY",
        None,
    );
    insert_transfer_log(
        &pool,
        "b-1",
        "src",
        Some("dst-b"),
        "FULL_KNOWLEDGE_BUNDLE",
        None,
    );

    let req = build_get_request(
        "operator",
        "/api/rtx/governance/transfers?destination=dst-a",
    );
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total"], 1);
    assert_eq!(body["transfers"][0]["destination_pseudonym"], "dst-a");
}

// ============================================================================
// Transfers Endpoint — Pagination
// ============================================================================

#[tokio::test]
async fn test_governance_transfers_pagination() {
    let (app, pool) = setup_app().await;
    register_operator(&pool, "operator");

    // Insert 5 entries
    for i in 0..5 {
        insert_transfer_log(
            &pool,
            &format!("b-{}", i),
            "src",
            None,
            "FULL_KNOWLEDGE_BUNDLE",
            None,
        );
    }

    // First page: limit=2, offset=0
    let req = build_get_request("operator", "/api/rtx/governance/transfers?limit=2&offset=0");
    let response = app.clone().oneshot(req).await.unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total"], 5);
    assert_eq!(body["transfers"].as_array().unwrap().len(), 2);
    assert_eq!(body["limit"], 2);
    assert_eq!(body["offset"], 0);

    // Second page: limit=2, offset=2
    let req = build_get_request("operator", "/api/rtx/governance/transfers?limit=2&offset=2");
    let response = app.clone().oneshot(req).await.unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total"], 5);
    assert_eq!(body["transfers"].as_array().unwrap().len(), 2);
    assert_eq!(body["offset"], 2);

    // Last page: offset=4
    let req = build_get_request("operator", "/api/rtx/governance/transfers?limit=2&offset=4");
    let response = app.oneshot(req).await.unwrap();
    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total"], 5);
    assert_eq!(body["transfers"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_governance_transfers_limit_capped_at_500() {
    let (app, pool) = setup_app().await;
    register_operator(&pool, "operator");

    let req = build_get_request("operator", "/api/rtx/governance/transfers?limit=9999");
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["limit"], 500);
}

// ============================================================================
// Summary Endpoint
// ============================================================================

#[tokio::test]
async fn test_governance_summary_empty() {
    let (app, pool) = setup_app().await;
    register_operator(&pool, "operator");

    let req = build_get_request("operator", "/api/rtx/governance/summary");
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total_transfers"], 0);
    assert_eq!(body["unique_bundles"], 0);
    assert_eq!(body["unique_sources"], 0);
    assert_eq!(body["unique_destinations"], 0);
    assert_eq!(body["redacted_transfers"], 0);
    assert!(body["by_scope"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_governance_summary_with_data() {
    let (app, pool) = setup_app().await;
    register_operator(&pool, "operator");

    // Publish entries (destination = NULL)
    insert_transfer_log(&pool, "b-1", "agent-a", None, "FULL_KNOWLEDGE_BUNDLE", None);
    insert_transfer_log(
        &pool,
        "b-2",
        "agent-b",
        None,
        "REFLECTION_SUMMARIES_ONLY",
        Some("reasoning_chain_stripped"),
    );

    // Delivery entries
    insert_transfer_log(
        &pool,
        "b-1",
        "agent-a",
        Some("agent-c"),
        "FULL_KNOWLEDGE_BUNDLE",
        None,
    );
    insert_transfer_log(
        &pool,
        "b-1",
        "agent-a",
        Some("agent-d"),
        "REFLECTION_SUMMARIES_ONLY",
        Some("reasoning_chain_stripped"),
    );
    insert_transfer_log(
        &pool,
        "b-2",
        "agent-b",
        Some("agent-c"),
        "REFLECTION_SUMMARIES_ONLY",
        Some("reasoning_chain_stripped"),
    );

    let req = build_get_request("operator", "/api/rtx/governance/summary");
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total_transfers"], 5);
    assert_eq!(body["unique_bundles"], 2); // b-1, b-2
    assert_eq!(body["unique_sources"], 2); // agent-a, agent-b
    assert_eq!(body["unique_destinations"], 2); // agent-c, agent-d
    assert_eq!(body["redacted_transfers"], 3); // 3 with reasoning_chain_stripped

    // Scope breakdown
    let by_scope = body["by_scope"].as_array().unwrap();
    assert_eq!(by_scope.len(), 2);

    // Find each scope entry (order is by count DESC)
    let reflection_entry = by_scope
        .iter()
        .find(|e| e["scope"] == "REFLECTION_SUMMARIES_ONLY")
        .expect("should have REFLECTION_SUMMARIES_ONLY entry");
    assert_eq!(reflection_entry["count"], 3);

    let full_entry = by_scope
        .iter()
        .find(|e| e["scope"] == "FULL_KNOWLEDGE_BUNDLE")
        .expect("should have FULL_KNOWLEDGE_BUNDLE entry");
    assert_eq!(full_entry["count"], 2);
}

// ============================================================================
// End-to-End: Publish then query governance
// ============================================================================

#[tokio::test]
async fn test_publish_then_governance_audit() {
    let (app, pool) = setup_app().await;

    // Register operator (can_moderate=1) who is also an agent
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active)
             VALUES (1, 'agent-op', 'AI_AGENT', 1, 1)",
            [],
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
            ) VALUES (1, 'agent-op', 'ALIGNED', 'FULL_KNOWLEDGE_BUNDLE', ?1, 1.0, datetime('now'))",
            [&contract_json],
        )
        .unwrap();
    }

    // Publish a bundle
    let bundle = serde_json::json!({
        "bundle_id": "governance-test-bundle",
        "source_pseudonym": "agent-op",
        "source_server": "http://localhost:3000",
        "domain_tags": ["rust", "crypto"],
        "summary": "Test summary for governance audit.",
        "reasoning_chain": "Step 1; Step 2.",
        "caveats": [],
        "created_at": 1700000000000_u64,
        "signature": "test-sig-123",
        "vrp_handshake_ref": "1:1:1"
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut publish_req = Request::builder()
        .uri("/api/rtx/publish")
        .method("POST")
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", "agent-op")
        .body(Body::from(bundle.to_string()))
        .unwrap();
    publish_req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.clone().oneshot(publish_req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Now query the governance transfers endpoint
    let req = build_get_request(
        "agent-op",
        "/api/rtx/governance/transfers?bundle_id=governance-test-bundle",
    );
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total"], 1);
    let transfer = &body["transfers"][0];
    assert_eq!(transfer["bundle_id"], "governance-test-bundle");
    assert_eq!(transfer["source_pseudonym"], "agent-op");
    assert!(transfer["destination_pseudonym"].is_null()); // Publish entry (no destination)
    assert_eq!(transfer["transfer_scope_applied"], "FULL_KNOWLEDGE_BUNDLE");

    // Query the summary
    let req = build_get_request("agent-op", "/api/rtx/governance/summary");
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();

    assert_eq!(body["total_transfers"], 1);
    assert_eq!(body["unique_bundles"], 1);
    assert_eq!(body["unique_sources"], 1);
}
