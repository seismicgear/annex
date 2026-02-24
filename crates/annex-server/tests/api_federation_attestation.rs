use annex_db::{create_pool, DbRuntimeSettings};
use annex_federation::AttestationRequest;
use annex_identity::MerkleTree;
use annex_server::{api::GetRootResponse, app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use rand::{rngs::OsRng, RngCore};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

// Mock loading vkey (dummy one is fine for this test unless we need to verify a real proof)
fn load_dummy_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
}

async fn setup_app() -> axum::Router {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Seed server
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local Server', '{}')",
        [],
    )
    .unwrap();

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: std::sync::Arc::new(std::sync::RwLock::new("http://localhost:3000".to_string())),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
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
    };

    app(state)
}

#[tokio::test]
async fn test_get_vrp_root() {
    let app = setup_app().await;
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/federation/vrp-root")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let resp: GetRootResponse = serde_json::from_slice(&body_bytes).unwrap();

    // Check root hex length (64 chars for 32 bytes hex)
    assert_eq!(resp.root_hex.len(), 64);
}

#[tokio::test]
async fn test_attest_membership_unknown_remote() {
    let app = setup_app().await;
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let payload = AttestationRequest {
        originating_server: "http://unknown.com".to_string(),
        topic: "annex:server:v1".to_string(),
        commitment: "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        proof: serde_json::json!({}), // Dummy proof
        participant_type: "HUMAN".to_string(),
        signature: "00".to_string(), // Dummy signature
    };

    let mut request = Request::builder()
        .uri("/api/federation/attest-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();

    // Should fail with 404 Unknown Remote
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_attest_membership_invalid_signature() {
    // Setup app with a known instance in DB
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Seed server
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local Server', '{}')",
        [],
    )
    .unwrap();

    // Seed instance with a key
    let mut csprng = OsRng;
    let mut bytes = [0u8; 32];
    csprng.fill_bytes(&mut bytes);
    let signing_key: SigningKey = SigningKey::from_bytes(&bytes);
    let verifying_key: VerifyingKey = signing_key.verifying_key();
    let public_key_hex = hex::encode(verifying_key.as_bytes());

    conn.execute(
        "INSERT INTO instances (base_url, public_key, label) VALUES (?1, ?2, 'Remote Server')",
        rusqlite::params!["http://remote.com", public_key_hex],
    )
    .unwrap();

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: std::sync::Arc::new(std::sync::RwLock::new("http://localhost:3000".to_string())),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
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
    };

    let app = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // Sign invalid message or modify signature
    let payload = AttestationRequest {
        originating_server: "http://remote.com".to_string(),
        topic: "annex:server:v1".to_string(),
        commitment: "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        proof: serde_json::json!({}),
        participant_type: "HUMAN".to_string(),
        signature: "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000".to_string(), // Invalid signature (64 bytes hex = 128 chars)
    };

    let mut request = Request::builder()
        .uri("/api/federation/attest-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();

    // Should fail with 401 (InvalidSignature is a client error)
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(body_str.contains("Invalid signature"));
}

#[tokio::test]
async fn test_attest_membership_valid_signature_fails_network() {
    // This tests that signature verification passes, but network call fails
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // Seed server
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local Server', '{}')",
        [],
    )
    .unwrap();

    // Seed instance with a key
    let mut csprng = OsRng;
    let mut bytes = [0u8; 32];
    csprng.fill_bytes(&mut bytes);
    let signing_key: SigningKey = SigningKey::from_bytes(&bytes);
    let verifying_key: VerifyingKey = signing_key.verifying_key();
    let public_key_hex = hex::encode(verifying_key.as_bytes());

    conn.execute(
        "INSERT INTO instances (base_url, public_key, label) VALUES (?1, ?2, 'Remote Server')",
        rusqlite::params!["http://localhost:9999", public_key_hex], // Port 9999 likely closed
    )
    .unwrap();

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: std::sync::Arc::new(std::sync::RwLock::new("http://localhost:3000".to_string())),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
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
    };

    let app = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let topic = "annex:server:v1".to_string();
    let commitment = "0000000000000000000000000000000000000000000000000000000000000001".to_string();
    let participant_type = "HUMAN".to_string();
    let message = format!("{}\n{}\n{}", topic, commitment, participant_type);
    let signature = signing_key.sign(message.as_bytes());
    let signature_hex = hex::encode(signature.to_bytes());

    let payload = AttestationRequest {
        originating_server: "http://localhost:9999".to_string(),
        topic,
        commitment,
        proof: serde_json::json!({}),
        participant_type,
        signature: signature_hex,
    };

    let mut request = Request::builder()
        .uri("/api/federation/attest-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();

    // Should fail with 500 (Network error)
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    // Network error message depends on OS but usually contains "connect" or "refused" or "error sending request"
    assert!(body_str.contains("Network error") || body_str.contains("error sending request"));
}
