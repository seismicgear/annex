//! Integration tests for `POST /api/federation/rtx` — cross-server RTX relay.
//!
//! Verifies that:
//! - Federated RTX bundles are accepted when the relaying server has a valid
//!   federation agreement with sufficient transfer scope.
//! - The server signature on the envelope is verified against the remote
//!   instance's public key.
//! - Bundles are stored with provenance metadata.
//! - Transfer scope is enforced (reasoning_chain stripped for ReflectionSummariesOnly).
//! - Subscribers with `accept_federated = true` receive deliveries.
//! - Various rejection cases work correctly (unknown server, no agreement,
//!   invalid signature, insufficient scope, etc.).

use annex_db::{create_pool, DbRuntimeSettings};
use annex_federation::FederatedRtxEnvelope;
use annex_identity::MerkleTree;
use annex_rtx::{BundleProvenance, ReflectionSummaryBundle};
use annex_server::{api_rtx::rtx_relay_signing_payload, app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use ed25519_dalek::{Signer, SigningKey};
use rand::rngs::OsRng;
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

fn load_dummy_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
}

struct TestEnv {
    pool: annex_db::DbPool,
    local_server_id: i64,
    remote_instance_id: i64,
    remote_signing_key: SigningKey,
    state: AppState,
}

fn setup_test_env(transfer_scope: &str) -> TestEnv {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    let policy = ServerPolicy::default();
    let policy_json = serde_json::to_string(&policy).unwrap();

    // Seed local server
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local Server', ?1)",
        rusqlite::params![policy_json],
    )
    .unwrap();
    let local_server_id = conn.last_insert_rowid();

    // Seed remote instance
    let mut csprng = OsRng;
    let remote_signing_key = SigningKey::generate(&mut csprng);
    let remote_public_key = remote_signing_key.verifying_key();
    let remote_public_key_hex = hex::encode(remote_public_key.as_bytes());

    conn.execute(
        "INSERT INTO instances (base_url, public_key, label, status) VALUES ('http://remote-server.com', ?1, 'Remote Server', 'ACTIVE')",
        rusqlite::params![remote_public_key_hex],
    ).unwrap();
    let remote_instance_id = conn.last_insert_rowid();

    // Seed federation agreement
    conn.execute(
        "INSERT INTO federation_agreements (
            local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active
        ) VALUES (?1, ?2, 'ALIGNED', ?3, '{}', 1)",
        rusqlite::params![local_server_id, remote_instance_id, transfer_scope],
    )
    .unwrap();

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let local_signing_key = Arc::new(SigningKey::generate(&mut csprng));

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
        server_id: local_server_id,
        signing_key: local_signing_key.clone(),
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

    TestEnv {
        pool,
        local_server_id,
        remote_instance_id,
        remote_signing_key,
        state,
    }
}

fn make_bundle(bundle_id: &str) -> ReflectionSummaryBundle {
    ReflectionSummaryBundle {
        bundle_id: bundle_id.to_string(),
        source_pseudonym: "agent-remote".to_string(),
        source_server: "http://remote-server.com".to_string(),
        domain_tags: vec!["rust".to_string(), "systems".to_string()],
        summary: "Rust's ownership model prevents data races.".to_string(),
        reasoning_chain: Some("Step 1: ownership; Step 2: borrow checker.".to_string()),
        caveats: vec!["Applies to safe Rust only".to_string()],
        created_at: 1700000000000,
        signature: "abcdef1234567890".to_string(),
        vrp_handshake_ref: "server1:instance1:agreement1".to_string(),
    }
}

fn sign_envelope(
    signing_key: &SigningKey,
    bundle: &ReflectionSummaryBundle,
    relaying_server: &str,
    origin_server: &str,
    relay_path: &[String],
) -> String {
    let payload = rtx_relay_signing_payload(
        &bundle.bundle_id,
        relaying_server,
        origin_server,
        relay_path,
    );
    let signature = signing_key.sign(payload.as_bytes());
    hex::encode(signature.to_bytes())
}

fn build_envelope(
    bundle: ReflectionSummaryBundle,
    signing_key: &SigningKey,
) -> FederatedRtxEnvelope {
    let relaying_server = "http://remote-server.com";
    let origin_server = bundle.source_server.clone();
    let relay_path = vec![relaying_server.to_string()];

    let signature = sign_envelope(
        signing_key,
        &bundle,
        relaying_server,
        &origin_server,
        &relay_path,
    );

    let provenance = BundleProvenance {
        origin_server,
        relay_path,
        bundle_id: bundle.bundle_id.clone(),
    };

    FederatedRtxEnvelope {
        bundle,
        provenance,
        relaying_server: relaying_server.to_string(),
        signature,
    }
}

fn build_request(envelope: &FederatedRtxEnvelope) -> Request<Body> {
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/federation/rtx")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(envelope).unwrap()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    req
}

async fn response_body(response: axum::http::Response<Body>) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Registers a local agent in the DB so it can receive relayed bundles.
fn register_local_agent(env: &TestEnv, pseudonym: &str, transfer_scope: &str) {
    let conn = env.pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
         VALUES (?1, ?2, 'AI_AGENT', 1)",
        rusqlite::params![env.local_server_id, pseudonym],
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
        ) VALUES (?1, ?2, 'ALIGNED', ?3, ?4, 1.0, datetime('now'))",
        rusqlite::params![
            env.local_server_id,
            pseudonym,
            transfer_scope,
            contract_json
        ],
    )
    .unwrap();
}

/// Creates an RTX subscription for a local agent.
fn create_subscription(
    env: &TestEnv,
    pseudonym: &str,
    accept_federated: bool,
    domain_filters: &[&str],
) {
    let conn = env.pool.get().unwrap();
    let filters_json = serde_json::to_string(&domain_filters).unwrap();
    let accept_fed: i32 = if accept_federated { 1 } else { 0 };
    conn.execute(
        "INSERT INTO rtx_subscriptions (server_id, subscriber_pseudonym, domain_filters_json, accept_federated)
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![env.local_server_id, pseudonym, filters_json, accept_fed],
    )
    .unwrap();
}

// ============================================================================
// Success Cases
// ============================================================================

#[tokio::test]
async fn test_receive_federated_rtx_success() {
    let env = setup_test_env("REFLECTION_SUMMARIES_ONLY");
    let app = app(env.state.clone());

    let bundle = make_bundle("bundle-fed-001");
    let envelope = build_envelope(bundle, &env.remote_signing_key);
    let req = build_request(&envelope);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response_body(response).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["bundleId"], "bundle-fed-001");

    // Verify bundle stored in DB
    let conn = env.pool.get().unwrap();
    let stored: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM rtx_bundles WHERE bundle_id = 'bundle-fed-001')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(stored, "bundle should be stored in rtx_bundles");

    // Verify provenance was stored
    let provenance: Option<String> = conn
        .query_row(
            "SELECT provenance_json FROM rtx_bundles WHERE bundle_id = 'bundle-fed-001'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(provenance.is_some(), "provenance should be stored");

    let prov: BundleProvenance = serde_json::from_str(provenance.as_ref().unwrap()).unwrap();
    assert_eq!(prov.origin_server, "http://remote-server.com");
    assert_eq!(prov.relay_path, vec!["http://remote-server.com"]);
    assert_eq!(prov.bundle_id, "bundle-fed-001");

    // Verify transfer log entry
    let log_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rtx_transfer_log WHERE bundle_id = 'bundle-fed-001'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(log_count, 1, "federated transfer should be logged");
}

#[tokio::test]
async fn test_receive_federated_rtx_strips_reasoning_for_reflection_scope() {
    let env = setup_test_env("REFLECTION_SUMMARIES_ONLY");
    let app = app(env.state.clone());

    let bundle = make_bundle("bundle-scope-001");
    let envelope = build_envelope(bundle, &env.remote_signing_key);
    let req = build_request(&envelope);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Verify reasoning_chain was stripped
    let conn = env.pool.get().unwrap();
    let reasoning: Option<String> = conn
        .query_row(
            "SELECT reasoning_chain FROM rtx_bundles WHERE bundle_id = 'bundle-scope-001'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        reasoning.is_none(),
        "reasoning_chain should be stripped for ReflectionSummariesOnly scope"
    );
}

#[tokio::test]
async fn test_receive_federated_rtx_preserves_reasoning_for_full_scope() {
    let env = setup_test_env("FULL_KNOWLEDGE_BUNDLE");
    let app = app(env.state.clone());

    let bundle = make_bundle("bundle-full-001");
    let envelope = build_envelope(bundle, &env.remote_signing_key);
    let req = build_request(&envelope);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Verify reasoning_chain is preserved
    let conn = env.pool.get().unwrap();
    let reasoning: Option<String> = conn
        .query_row(
            "SELECT reasoning_chain FROM rtx_bundles WHERE bundle_id = 'bundle-full-001'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(
        reasoning.is_some(),
        "reasoning_chain should be preserved for FullKnowledgeBundle scope"
    );
}

#[tokio::test]
async fn test_receive_federated_rtx_delivers_to_accepting_subscribers() {
    let env = setup_test_env("FULL_KNOWLEDGE_BUNDLE");
    register_local_agent(&env, "agent-local-sub", "REFLECTION_SUMMARIES_ONLY");
    create_subscription(&env, "agent-local-sub", true, &[]);

    let app = app(env.state.clone());

    let bundle = make_bundle("bundle-deliver-001");
    let envelope = build_envelope(bundle, &env.remote_signing_key);
    let req = build_request(&envelope);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response_body(response).await;
    assert_eq!(
        body["delivered_to"], 1,
        "should deliver to one federated subscriber"
    );

    // Verify delivery log
    let conn = env.pool.get().unwrap();
    let delivery_log: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rtx_transfer_log WHERE bundle_id = 'bundle-deliver-001' AND destination_pseudonym = 'agent-local-sub'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(delivery_log, 1, "delivery should be logged");
}

#[tokio::test]
async fn test_receive_federated_rtx_skips_non_federated_subscribers() {
    let env = setup_test_env("FULL_KNOWLEDGE_BUNDLE");
    register_local_agent(&env, "agent-nofed", "REFLECTION_SUMMARIES_ONLY");
    create_subscription(&env, "agent-nofed", false, &[]); // accept_federated = false

    let app = app(env.state.clone());

    let bundle = make_bundle("bundle-nofed-001");
    let envelope = build_envelope(bundle, &env.remote_signing_key);
    let req = build_request(&envelope);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response_body(response).await;
    assert_eq!(
        body["delivered_to"], 0,
        "should not deliver to subscriber with accept_federated = false"
    );
}

#[tokio::test]
async fn test_receive_federated_rtx_respects_domain_filters() {
    let env = setup_test_env("FULL_KNOWLEDGE_BUNDLE");
    register_local_agent(&env, "agent-filter", "REFLECTION_SUMMARIES_ONLY");
    create_subscription(&env, "agent-filter", true, &["python", "ml"]); // doesn't match "rust", "systems"

    let app = app(env.state.clone());

    let bundle = make_bundle("bundle-filter-001");
    let envelope = build_envelope(bundle, &env.remote_signing_key);
    let req = build_request(&envelope);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body = response_body(response).await;
    assert_eq!(
        body["delivered_to"], 0,
        "should not deliver when domain filters don't match"
    );
}

// ============================================================================
// Idempotency
// ============================================================================

#[tokio::test]
async fn test_receive_federated_rtx_idempotent_on_duplicate() {
    let env = setup_test_env("REFLECTION_SUMMARIES_ONLY");
    let app = app(env.state.clone());

    let bundle = make_bundle("bundle-dup-001");
    let envelope = build_envelope(bundle, &env.remote_signing_key);

    // First request
    let req1 = build_request(&envelope);
    let resp1 = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(resp1.status(), StatusCode::OK);

    // Second request (same bundle_id)
    let req2 = build_request(&envelope);
    let resp2 = app.oneshot(req2).await.unwrap();
    assert_eq!(resp2.status(), StatusCode::OK);

    let body = response_body(resp2).await;
    assert_eq!(
        body["delivered_to"], 0,
        "duplicate bundle should be silently accepted with 0 deliveries"
    );

    // Verify only one row in DB
    let conn = env.pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rtx_bundles WHERE bundle_id = 'bundle-dup-001'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1, "should store only one copy");
}

// ============================================================================
// Rejection Cases
// ============================================================================

#[tokio::test]
async fn test_receive_federated_rtx_rejects_unknown_server() {
    let env = setup_test_env("REFLECTION_SUMMARIES_ONLY");
    let app = app(env.state.clone());

    let bundle = make_bundle("bundle-unknown-001");
    let provenance = BundleProvenance {
        origin_server: "http://unknown-server.com".to_string(),
        relay_path: vec!["http://unknown-server.com".to_string()],
        bundle_id: bundle.bundle_id.clone(),
    };

    let envelope = FederatedRtxEnvelope {
        bundle,
        provenance,
        relaying_server: "http://unknown-server.com".to_string(),
        signature: "deadbeef".to_string(),
    };

    let req = build_request(&envelope);
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "should reject unknown relaying server"
    );
}

#[tokio::test]
async fn test_receive_federated_rtx_rejects_no_transfer_scope() {
    let env = setup_test_env("NO_TRANSFER");
    let app = app(env.state.clone());

    let bundle = make_bundle("bundle-notransfer-001");
    let envelope = build_envelope(bundle, &env.remote_signing_key);
    let req = build_request(&envelope);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "should reject when federation agreement has NoTransfer scope"
    );
}

#[tokio::test]
async fn test_receive_federated_rtx_rejects_invalid_signature() {
    let env = setup_test_env("REFLECTION_SUMMARIES_ONLY");
    let app = app(env.state.clone());

    let bundle = make_bundle("bundle-badsig-001");

    // Sign with a different key (not the remote instance's key)
    let wrong_key = SigningKey::generate(&mut OsRng);
    let envelope = build_envelope(bundle, &wrong_key);

    let req = build_request(&envelope);
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED, // FederationError::InvalidSignature maps to 401
        "should reject invalid server signature"
    );
}

#[tokio::test]
async fn test_receive_federated_rtx_rejects_inactive_instance() {
    let env = setup_test_env("REFLECTION_SUMMARIES_ONLY");

    // Mark instance as INACTIVE
    {
        let conn = env.pool.get().unwrap();
        conn.execute(
            "UPDATE instances SET status = 'INACTIVE' WHERE id = ?1",
            rusqlite::params![env.remote_instance_id],
        )
        .unwrap();
    }

    let app = app(env.state.clone());
    let bundle = make_bundle("bundle-inactive-001");
    let envelope = build_envelope(bundle, &env.remote_signing_key);
    let req = build_request(&envelope);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "should reject relays from inactive instances"
    );
}

#[tokio::test]
async fn test_receive_federated_rtx_rejects_no_agreement() {
    let env = setup_test_env("REFLECTION_SUMMARIES_ONLY");

    // Deactivate federation agreement
    {
        let conn = env.pool.get().unwrap();
        conn.execute(
            "UPDATE federation_agreements SET active = 0 WHERE remote_instance_id = ?1",
            rusqlite::params![env.remote_instance_id],
        )
        .unwrap();
    }

    let app = app(env.state.clone());
    let bundle = make_bundle("bundle-noagree-001");
    let envelope = build_envelope(bundle, &env.remote_signing_key);
    let req = build_request(&envelope);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "should reject when no active federation agreement"
    );
}

#[tokio::test]
async fn test_receive_federated_rtx_rejects_empty_summary() {
    let env = setup_test_env("REFLECTION_SUMMARIES_ONLY");
    let app = app(env.state.clone());

    let mut bundle = make_bundle("bundle-nosummary-001");
    bundle.summary = String::new();

    let envelope = build_envelope(bundle, &env.remote_signing_key);
    let req = build_request(&envelope);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::FORBIDDEN,
        "should reject bundles with empty summary"
    );
}

// ============================================================================
// Unit tests for rtx_relay_signing_payload
// ============================================================================

#[test]
fn test_signing_payload_deterministic() {
    let p1 = rtx_relay_signing_payload(
        "bundle-123",
        "http://relay.com",
        "http://origin.com",
        &["http://relay.com".to_string()],
    );

    let p2 = rtx_relay_signing_payload(
        "bundle-123",
        "http://relay.com",
        "http://origin.com",
        &["http://relay.com".to_string()],
    );

    assert_eq!(p1, p2, "signing payload should be deterministic");
}

#[test]
fn test_signing_payload_includes_all_fields() {
    let payload = rtx_relay_signing_payload(
        "bundle-abc",
        "http://relay.com",
        "http://origin.com",
        &["http://hop1.com".to_string(), "http://hop2.com".to_string()],
    );

    assert!(payload.contains("bundle-abc"));
    assert!(payload.contains("http://relay.com"));
    assert!(payload.contains("http://origin.com"));
    assert!(payload.contains("http://hop1.com|http://hop2.com"));
}

#[test]
fn test_signing_payload_empty_relay_path() {
    let payload =
        rtx_relay_signing_payload("bundle-xyz", "http://relay.com", "http://origin.com", &[]);

    assert_eq!(payload, "bundle-xyz\nhttp://relay.com\nhttp://origin.com\n");
}
