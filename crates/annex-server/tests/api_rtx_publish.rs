//! Integration tests for `POST /api/rtx/publish`.

mod common;

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
    let policy = ServerPolicy {
        allow_full_knowledge: true,
        ..ServerPolicy::default()
    };

    let state = AppState {
        pool: pool.clone(),
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
        policy: Arc::new(RwLock::new(policy)),
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
        federation_transport: Default::default(),
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
        metrics: Default::default(),
    };

    (app(state), pool)
}

/// Exercise the actual handshake handler before inserting the authentication
/// fixture. The explicit scope override below isolates existing per-scope gate
/// tests; negotiation itself is covered by api_vrp_handshake and the live lane.
async fn register_agent_contract(
    app: &axum::Router,
    pool: &annex_db::DbPool,
    pseudonym: &str,
    transfer_scope: &str,
    redacted_topics: &[&str],
    signing_pubkey: Option<&str>,
) {
    let handshake = annex_vrp::VrpFederationHandshake {
        anchor_snapshot: annex_vrp::VrpAnchorSnapshot::new(&[], &[]).unwrap(),
        capability_contract: annex_vrp::VrpCapabilitySharingContract {
            required_capabilities: vec![],
            offered_capabilities: vec!["TEXT".into(), "VRP".into()],
            redacted_topics: redacted_topics.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        },
        scorer: None,
    };
    let mut req = Request::builder().uri("/api/vrp/agent-handshake").method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::json!({"pseudonymId": pseudonym, "handshake": handshake, "signingPubkey": signing_pubkey}).to_string())).unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let report: Value = serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(report["handshakeRef"]
        .as_str()
        .unwrap()
        .starts_with("vrp:1:"));
    let conn = pool.get().unwrap();
    conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, ?1, 'AI_AGENT', 1)", [pseudonym]).unwrap();
    conn.execute("UPDATE agent_registrations SET transfer_scope = ?1 WHERE server_id = 1 AND pseudonym_id = ?2", rusqlite::params![transfer_scope, pseudonym]).unwrap();
}

async fn register_agent(
    app: &axum::Router,
    pool: &annex_db::DbPool,
    pseudonym: &str,
    transfer_scope: &str,
) {
    register_agent_contract(app, pool, pseudonym, transfer_scope, &[], None).await;
}
async fn register_agent_with_pubkey(
    app: &axum::Router,
    pool: &annex_db::DbPool,
    pseudonym: &str,
    transfer_scope: &str,
    key: &str,
) {
    register_agent_contract(app, pool, pseudonym, transfer_scope, &[], Some(key)).await;
}
async fn register_agent_with_redactions(
    app: &axum::Router,
    pool: &annex_db::DbPool,
    pseudonym: &str,
    transfer_scope: &str,
    topics: &[&str],
) {
    register_agent_contract(app, pool, pseudonym, transfer_scope, topics, None).await;
}
fn current_ref(pool: &annex_db::DbPool, pseudonym: &str) -> String {
    use rusqlite::OptionalExtension;
    let id: Option<i64> = pool.get().unwrap().query_row("SELECT current_handshake_id FROM agent_registrations WHERE server_id = 1 AND pseudonym_id = ?1", [pseudonym], |r| r.get(0)).optional().unwrap();
    format!("vrp:1:{}", id.unwrap_or(0))
}

fn make_bundle(pool: &annex_db::DbPool, pseudonym: &str) -> Value {
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
        "vrp_handshake_ref": current_ref(pool, pseudonym)
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
    register_agent(&app, &pool, "agent-pub", "REFLECTION_SUMMARIES_ONLY").await;

    let bundle = make_bundle(&pool, "agent-pub");
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
    register_agent(&app, &pool, "agent-full", "FULL_KNOWLEDGE_BUNDLE").await;

    let bundle = make_bundle(&pool, "agent-full");
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
    let (app, pool) = setup_app().await;

    let bundle = make_bundle(&pool, "agent-noauth");

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
    register_agent(&app, &pool, "agent-notransfer", "NO_TRANSFER").await;

    let bundle = make_bundle(&pool, "agent-notransfer");
    let req = build_publish_request("agent-notransfer", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_publish_rejects_mismatched_pseudonym() {
    let (app, pool) = setup_app().await;
    register_agent(&app, &pool, "agent-real", "REFLECTION_SUMMARIES_ONLY").await;

    // Bundle claims source_pseudonym is "agent-fake" but auth is "agent-real"
    let mut bundle = make_bundle(&pool, "agent-fake");
    // Keep the pseudonym as "agent-fake" in the bundle
    bundle["source_pseudonym"] = serde_json::json!("agent-fake");

    let req = build_publish_request("agent-real", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_publish_rejects_wrong_source_server() {
    let (app, pool) = setup_app().await;
    register_agent(&app, &pool, "agent-srv", "REFLECTION_SUMMARIES_ONLY").await;

    let mut bundle = make_bundle(&pool, "agent-srv");
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

    let bundle = make_bundle(&pool, "agent-noreg");
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
    register_agent(&app, &pool, "agent-val", "REFLECTION_SUMMARIES_ONLY").await;

    let mut bundle = make_bundle(&pool, "agent-val");
    bundle["summary"] = serde_json::json!("");

    let req = build_publish_request("agent-val", &bundle);

    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_publish_rejects_duplicate_bundle_id() {
    let (app, pool) = setup_app().await;
    register_agent(&app, &pool, "agent-dup", "REFLECTION_SUMMARIES_ONLY").await;

    let bundle = make_bundle(&pool, "agent-dup");

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
        &app,
        &pool,
        "agent-redact",
        "REFLECTION_SUMMARIES_ONLY",
        &["politics"],
    )
    .await;

    // Bundle has domain_tag "politics" which is redacted
    let mut bundle = make_bundle(&pool, "agent-redact");
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
        &app,
        &pool,
        "agent-ok",
        "REFLECTION_SUMMARIES_ONLY",
        &["politics"],
    )
    .await;

    // Bundle has "rust" and "systems" which are not redacted
    let bundle = make_bundle(&pool, "agent-ok");

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
    register_agent(&app, &pool, "agent-sender", "FULL_KNOWLEDGE_BUNDLE").await;
    register_agent(&app, &pool, "agent-receiver", "REFLECTION_SUMMARIES_ONLY").await;

    // Create a subscription for agent-receiver
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO rtx_subscriptions (server_id, subscriber_pseudonym, domain_filters_json)
         VALUES (1, 'agent-receiver', '[]')",
        [],
    )
    .unwrap();
    drop(conn);

    let bundle = make_bundle(&pool, "agent-sender");
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
    register_agent(&app, &pool, "agent-pub-df", "FULL_KNOWLEDGE_BUNDLE").await;
    register_agent(&app, &pool, "agent-sub-df", "REFLECTION_SUMMARIES_ONLY").await;

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
    let bundle = make_bundle(&pool, "agent-pub-df");
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
    register_agent(&app, &pool, "agent-pub-corrupt", "FULL_KNOWLEDGE_BUNDLE").await;
    register_agent(
        &app,
        &pool,
        "agent-sub-corrupt",
        "REFLECTION_SUMMARIES_ONLY",
    )
    .await;
    register_agent(&app, &pool, "agent-sub-good", "REFLECTION_SUMMARIES_ONLY").await;

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

    let bundle = make_bundle(&pool, "agent-pub-corrupt");
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
    register_agent(&app, &pool, "agent-pub-scope", "FULL_KNOWLEDGE_BUNDLE").await;

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

    let bundle = make_bundle(&pool, "agent-pub-scope");
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

// ============================================================================
// Per-agent author signature enforcement (AUDIT P4-FED-1)
// ============================================================================

/// Builds a bundle and signs it with `sk` using the canonical author payload
/// (independently re-derived here via the public `author_signing_payload`, so
/// this proves the server's verifier matches an external signer).
fn signed_bundle(
    pool: &annex_db::DbPool,
    pseudonym: &str,
    sk: &ed25519_dalek::SigningKey,
) -> Value {
    use ed25519_dalek::Signer;
    use sha2::{Digest, Sha256};

    let mut bundle = annex_rtx::ReflectionSummaryBundle {
        bundle_id: format!("bundle-{}", uuid::Uuid::new_v4()),
        source_pseudonym: pseudonym.to_string(),
        source_server: "http://localhost:3000".to_string(),
        domain_tags: vec!["rust".into(), "systems".into()],
        summary: "Rust ownership prevents data races at compile time.".into(),
        reasoning_chain: Some("ownership; borrow checker; lifetimes".into()),
        caveats: vec!["safe Rust only".into()],
        created_at: 1_700_000_000_000,
        signature: String::new(),
        vrp_handshake_ref: current_ref(pool, pseudonym),
    };
    let digest = Sha256::digest(annex_rtx::author_signing_payload(&bundle).as_bytes());
    bundle.signature = hex::encode(sk.sign(&digest).to_bytes());
    serde_json::to_value(&bundle).unwrap()
}

#[tokio::test]
async fn test_publish_accepts_valid_author_signature() {
    let (app, pool) = setup_app().await;
    let sk = ed25519_dalek::SigningKey::from_bytes(&[3u8; 32]);
    let pubkey_hex = hex::encode(sk.verifying_key().to_bytes());
    register_agent_with_pubkey(
        &app,
        &pool,
        "agent-signed",
        "FULL_KNOWLEDGE_BUNDLE",
        &pubkey_hex,
    )
    .await;

    let bundle = signed_bundle(&pool, "agent-signed", &sk);
    let req = build_publish_request("agent-signed", &bundle);
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a correctly author-signed bundle must publish"
    );
}

#[tokio::test]
async fn test_publish_rejects_tampered_content_when_pubkey_registered() {
    let (app, pool) = setup_app().await;
    let sk = ed25519_dalek::SigningKey::from_bytes(&[4u8; 32]);
    let pubkey_hex = hex::encode(sk.verifying_key().to_bytes());
    register_agent_with_pubkey(
        &app,
        &pool,
        "agent-tamper",
        "FULL_KNOWLEDGE_BUNDLE",
        &pubkey_hex,
    )
    .await;

    // Sign a bundle, then alter the summary AFTER signing — the author
    // signature no longer covers the content the server receives.
    let mut bundle = signed_bundle(&pool, "agent-tamper", &sk);
    bundle["summary"] = serde_json::json!("MALICIOUSLY rewritten content");
    let req = build_publish_request("agent-tamper", &bundle);
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "tampered content must fail author-signature verification"
    );
}

#[tokio::test]
async fn test_publish_rejects_garbage_signature_when_pubkey_registered() {
    let (app, pool) = setup_app().await;
    let sk = ed25519_dalek::SigningKey::from_bytes(&[5u8; 32]);
    let pubkey_hex = hex::encode(sk.verifying_key().to_bytes());
    register_agent_with_pubkey(
        &app,
        &pool,
        "agent-garbage",
        "FULL_KNOWLEDGE_BUNDLE",
        &pubkey_hex,
    )
    .await;

    // A structurally-valid but cryptographically-bogus signature.
    let mut bundle = signed_bundle(&pool, "agent-garbage", &sk);
    bundle["signature"] = serde_json::json!(hex::encode([0u8; 64]));
    let req = build_publish_request("agent-garbage", &bundle);
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_publish_legacy_agent_without_pubkey_still_works() {
    // Backward compat: an agent that never advertised a signing key keeps the
    // legacy structural-only check (no author signature enforced).
    let (app, pool) = setup_app().await;
    register_agent(&app, &pool, "agent-legacy", "FULL_KNOWLEDGE_BUNDLE").await;
    let bundle = make_bundle(&pool, "agent-legacy");
    let req = build_publish_request("agent-legacy", &bundle);
    let response = app.oneshot(req).await.unwrap();
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "legacy agent with no pubkey on file must still publish"
    );
}

#[tokio::test]
async fn publish_requires_current_accepted_handshake_owned_by_author() {
    let (app, pool) = setup_app().await;
    register_agent(&app, &pool, "author-ref", "FULL_KNOWLEDGE_BUNDLE").await;
    register_agent(&app, &pool, "other-ref", "FULL_KNOWLEDGE_BUNDLE").await;
    for reference in ["vrp:1:999999".to_string(), current_ref(&pool, "other-ref")] {
        let mut bundle = make_bundle(&pool, "author-ref");
        bundle["vrp_handshake_ref"] = Value::String(reference);
        let response = app
            .clone()
            .oneshot(build_publish_request("author-ref", &bundle))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let mut bundle = make_bundle(&pool, "author-ref");
    // A migrated registration without a historical accepted decision must not
    // gain one from the fact it was already active before this release.
    pool.get().unwrap().execute("UPDATE agent_registrations SET current_handshake_id = NULL WHERE pseudonym_id = 'author-ref'", []).unwrap();
    bundle["bundle_id"] = serde_json::json!(uuid::Uuid::new_v4().to_string());
    let response = app
        .oneshot(build_publish_request("author-ref", &bundle))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("current accepted VRP handshake"));
    let count: i64 = pool
        .get()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM rtx_bundles", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn actual_rehandshake_invalidates_previous_reference_and_profile_exposes_new_one() {
    let (app, pool) = setup_app().await;
    register_agent(&app, &pool, "author-fresh", "FULL_KNOWLEDGE_BUNDLE").await;
    let old_bundle = make_bundle(&pool, "author-fresh");
    let token = annex_server::api_ws::generate_session_token("author-fresh", &[0; 32], 3600, 0);
    let handshake = annex_vrp::VrpFederationHandshake {
        anchor_snapshot: annex_vrp::VrpAnchorSnapshot::new(&[], &[]).unwrap(),
        capability_contract: annex_vrp::VrpCapabilitySharingContract {
            required_capabilities: vec![],
            offered_capabilities: vec!["TEXT".into(), "VRP".into()],
            redacted_topics: vec![],
            ..Default::default()
        },
        scorer: None,
    };
    let mut req = Request::builder()
        .uri("/api/vrp/agent-handshake")
        .method("POST")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(
            serde_json::json!({"pseudonymId":"author-fresh", "handshake":handshake}).to_string(),
        ))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    let res = app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let report: Value = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_ne!(report["handshakeRef"], old_bundle["vrp_handshake_ref"]);
    assert_eq!(report["handshakeRef"], current_ref(&pool, "author-fresh"));
    assert_eq!(
        app.clone()
            .oneshot(build_publish_request("author-fresh", &old_bundle))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
    let fresh = make_bundle(&pool, "author-fresh");
    assert_eq!(
        app.clone()
            .oneshot(build_publish_request("author-fresh", &fresh))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let mut req = Request::builder()
        .uri("/api/agents/author-fresh")
        .header("X-Annex-Pseudonym", "author-fresh")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 12345))));
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let profile: Value = serde_json::from_slice(
        &axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(profile["handshakeRef"], report["handshakeRef"]);
}

#[tokio::test]
async fn outbox_failure_rolls_back_bundle_and_every_transfer_record() {
    let (app, pool) = setup_app().await;
    register_agent(&app, &pool, "atomic-author", "REFLECTION_SUMMARIES_ONLY").await;
    let conn = pool.get().unwrap();
    conn.execute("INSERT INTO instances (base_url, public_key, label, status) VALUES ('https://8.8.8.8', ?1, 'Unit peer', 'ACTIVE')", ["ab".repeat(32)]).unwrap();
    let peer = conn.last_insert_rowid();
    conn.execute("INSERT INTO federation_agreements (local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active) VALUES (1, ?1, 'ALIGNED', 'FULL_KNOWLEDGE_BUNDLE', '{}', 1)", [peer]).unwrap();
    conn.execute_batch("CREATE TRIGGER reject_rtx_queue BEFORE INSERT ON federation_outbox WHEN NEW.envelope_kind = 'rtx' BEGIN SELECT RAISE(ABORT, 'controlled queue write failure'); END;").unwrap();
    drop(conn);
    let bundle = make_bundle(&pool, "atomic-author");
    let response = app
        .clone()
        .oneshot(build_publish_request("atomic-author", &bundle))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let conn = pool.get().unwrap();
    for table in ["rtx_bundles", "rtx_transfer_log", "federation_outbox"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(
            count, 0,
            "{table} must roll back with the failed queue write"
        );
    }
    conn.execute_batch("DROP TRIGGER reject_rtx_queue").unwrap();
    drop(conn);
    assert_eq!(
        app.oneshot(build_publish_request("atomic-author", &bundle))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    let conn = pool.get().unwrap();
    let (kind, id, source, channel): (String, String, Option<String>, Option<String>) = conn.query_row(
        "SELECT envelope_kind, message_id, source_message_id, channel_id FROM federation_outbox", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap();
    assert_eq!(kind, "rtx");
    assert_eq!(id, format!("rtx:{}", bundle["bundle_id"].as_str().unwrap()));
    assert_eq!((source, channel), (None, None));
}
