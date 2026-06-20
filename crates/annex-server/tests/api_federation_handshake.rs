mod common;

use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use annex_vrp::{
    VrpAlignmentStatus, VrpAnchorSnapshot, VrpCapabilitySharingContract, VrpFederationHandshake,
    VrpValidationReport,
};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use ed25519_dalek::{Signer, SigningKey};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

/// Generates a remote signing key and returns both the key and the hex-encoded public key.
fn generate_remote_key() -> (SigningKey, String) {
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    let pub_hex = hex::encode(key.verifying_key().as_bytes());
    (key, pub_hex)
}

/// Signs a federation handshake payload with the given key.
fn sign_handshake(key: &SigningKey, base_url: &str, handshake: &VrpFederationHandshake) -> String {
    let handshake_json = serde_json::to_string(handshake).unwrap();
    let signing_payload = format!("{base_url}\n{handshake_json}");
    let signature = key.sign(signing_payload.as_bytes());
    hex::encode(signature.to_bytes())
}

async fn setup_app() -> (axum::Router, annex_db::DbPool, SigningKey) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Insert a server row
    conn.execute(
        "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'test-server', 'Test Server', '{}')",
        [],
    )
    .unwrap();

    // Generate a proper Ed25519 key for the remote instance
    let (remote_key, remote_pub_hex) = generate_remote_key();

    // Insert a remote instance with the real public key
    conn.execute(
        "INSERT INTO instances (id, base_url, public_key, label, status) VALUES (10, 'https://remote.example.com', ?1, 'Remote Instance', 'ACTIVE')",
        rusqlite::params![remote_pub_hex],
    ).unwrap();

    drop(conn); // Return connection to pool

    let tree = MerkleTree::new(20).unwrap();
    let policy = ServerPolicy::default();

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: common::load_vkey_or_dummy(),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
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
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
    };

    (app(state), pool, remote_key)
}

#[tokio::test]
async fn test_federation_handshake_success() {
    let (app, pool, remote_key) = setup_app().await;

    // 1. Prepare Payload
    let anchor = VrpAnchorSnapshot::new(&[], &[]).unwrap(); // Matches default policy
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
        redacted_topics: vec![],
    };
    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };

    let base_url = "https://remote.example.com";
    let signature = sign_handshake(&remote_key, base_url, &handshake);

    let payload = serde_json::json!({
        "base_url": base_url,
        "signature": signature,
        "anchor_snapshot": handshake.anchor_snapshot,
        "capability_contract": handshake.capability_contract
    });

    // 2. Send Request
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/federation/handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();

    // 3. Verify Response
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let report: VrpValidationReport = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(report.alignment_status, VrpAlignmentStatus::Aligned);

    // 4. Verify DB
    let conn = pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM federation_agreements WHERE remote_instance_id = 10",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_federation_handshake_unknown_instance() {
    let (app, _, remote_key) = setup_app().await;

    // 1. Prepare Payload with unknown URL
    let anchor = VrpAnchorSnapshot::new(&[], &[]).unwrap();
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
        redacted_topics: vec![],
    };
    let handshake = VrpFederationHandshake {
        anchor_snapshot: anchor,
        capability_contract: contract,
    };

    let base_url = "https://unknown.example.com";
    let signature = sign_handshake(&remote_key, base_url, &handshake);

    let payload = serde_json::json!({
        "base_url": base_url,
        "signature": signature,
        "anchor_snapshot": handshake.anchor_snapshot,
        "capability_contract": handshake.capability_contract
    });

    // 2. Send Request
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut req = Request::builder()
        .uri("/api/federation/handshake")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(payload.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(req).await.unwrap();

    // 3. Verify Response
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
