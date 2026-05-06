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
        membership_vkey_v2: None,
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
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
        participant_type: "AI_AGENT".to_string(),
        signature: "00".to_string(), // Dummy signature
        protocol_version: None,
        public_signals: None,
        nullifier_hex: None,
        topic_hash_hex: None,
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
        membership_vkey_v2: None,
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
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
    };

    let app = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // Sign invalid message or modify signature
    let payload = AttestationRequest {
        originating_server: "http://remote.com".to_string(),
        topic: "annex:server:v1".to_string(),
        commitment: "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        proof: serde_json::json!({}),
        participant_type: "AI_AGENT".to_string(),
        signature: "00000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000".to_string(), // Invalid signature (64 bytes hex = 128 chars)
        protocol_version: None,
        public_signals: None,
        nullifier_hex: None,
        topic_hash_hex: None,
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
        membership_vkey_v2: None,
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
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
    };

    let app = app(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let topic = "annex:server:v1".to_string();
    let commitment = "0000000000000000000000000000000000000000000000000000000000000001".to_string();
    let participant_type = "AI_AGENT".to_string();
    let message = format!("{topic}\n{commitment}\n{participant_type}");
    let signature = signing_key.sign(message.as_bytes());
    let signature_hex = hex::encode(signature.to_bytes());

    let payload = AttestationRequest {
        originating_server: "http://localhost:9999".to_string(),
        topic,
        commitment,
        proof: serde_json::json!({}),
        participant_type,
        signature: signature_hex,
        protocol_version: None,
        public_signals: None,
        nullifier_hex: None,
        topic_hash_hex: None,
    };

    let mut request = Request::builder()
        .uri("/api/federation/attest-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();

    // After the SSRF guard was added, attest-membership refuses to make
    // outbound calls to private/loopback hosts: a `localhost` peer URL is
    // now rejected with `403 Forbidden` BEFORE the network call happens.
    // The previous behaviour ("network error → 500") is no longer
    // reachable on a private peer URL.
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(
        body_str.contains("private or reserved"),
        "expected SSRF rejection error, got: {body_str}"
    );
}

#[tokio::test]
async fn test_attest_membership_rejects_human_participant_type() {
    let app = setup_app().await;
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let payload = AttestationRequest {
        originating_server: "http://any-server.com".to_string(),
        topic: "annex:server:v1".to_string(),
        commitment: "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        proof: serde_json::json!({}),
        participant_type: "HUMAN".to_string(),
        signature: "00".to_string(),
        protocol_version: None,
        public_signals: None,
        nullifier_hex: None,
        topic_hash_hex: None,
    };

    let mut request = Request::builder()
        .uri("/api/federation/attest-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();

    // HUMAN participant_type must be rejected before any other processing
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(
        body_str.contains("HUMAN participant_type is not permitted"),
        "Expected HUMAN rejection error, got: {body_str}"
    );
}

// ---------------------------------------------------------------------------
// v2 dispatch tests
// ---------------------------------------------------------------------------
//
// The federation attest-membership orchestration must dispatch to the v1 or
// v2 verifier based on `protocol_version`. These tests cover the input-shape
// gates that fire BEFORE the proof is verified — they don't need a real v2
// proof to exercise the dispatch logic.

/// Helper that seeds `instances` with a known signing key and returns
/// `(app, signing_key, base_url)` for v2 attestation tests.
async fn setup_app_with_known_instance(
    base_url: &str,
    membership_vkey_v2: Option<Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>>>,
) -> (axum::Router, SigningKey) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES ('local', 'Local Server', '{}')",
        [],
    )
    .unwrap();

    let mut csprng = OsRng;
    let mut bytes = [0u8; 32];
    csprng.fill_bytes(&mut bytes);
    let signing_key: SigningKey = SigningKey::from_bytes(&bytes);
    let verifying_key: VerifyingKey = signing_key.verifying_key();
    let public_key_hex = hex::encode(verifying_key.as_bytes());

    conn.execute(
        "INSERT INTO instances (base_url, public_key, label) VALUES (?1, ?2, 'Remote Server')",
        rusqlite::params![base_url, public_key_hex],
    )
    .unwrap();

    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_dummy_vkey(),
        membership_vkey_v2,
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
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
    };

    (app(state), signing_key)
}

#[tokio::test]
async fn test_attest_membership_v2_rejected_when_v2_not_enabled() {
    // v2 attestation against a server that has not loaded the v2 vkey
    // (membership_vkey_v2 = None) must be rejected with 403 Forbidden,
    // not silently downgraded to v1.
    let (app, _signing_key) =
        setup_app_with_known_instance("http://remote.example.com", None).await;
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let payload = AttestationRequest {
        originating_server: "http://remote.example.com".to_string(),
        topic: "annex:server:v1".to_string(),
        commitment: "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        proof: serde_json::json!({}),
        participant_type: "AI_AGENT".to_string(),
        signature: "00".to_string(),
        protocol_version: Some("v2".to_string()),
        public_signals: Some(vec![
            "0".to_string(),
            "1".to_string(),
            "2".to_string(),
            "3".to_string(),
        ]),
        nullifier_hex: Some(
            "0000000000000000000000000000000000000000000000000000000000000002".to_string(),
        ),
        topic_hash_hex: Some(
            "0000000000000000000000000000000000000000000000000000000000000003".to_string(),
        ),
    };

    let mut request = Request::builder()
        .uri("/api/federation/attest-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(
        body_str.contains("v2 is not enabled"),
        "Expected v2-disabled error, got: {body_str}"
    );
}

#[tokio::test]
async fn test_attest_membership_unknown_protocol_version_rejected() {
    let (app, _signing_key) =
        setup_app_with_known_instance("http://remote.example.com", None).await;
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let payload = AttestationRequest {
        originating_server: "http://remote.example.com".to_string(),
        topic: "annex:server:v1".to_string(),
        commitment: "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        proof: serde_json::json!({}),
        participant_type: "AI_AGENT".to_string(),
        signature: "00".to_string(),
        protocol_version: Some("v99".to_string()),
        public_signals: None,
        nullifier_hex: None,
        topic_hash_hex: None,
    };

    let mut request = Request::builder()
        .uri("/api/federation/attest-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(
        body_str.contains("unsupported protocol_version"),
        "Expected unsupported version error, got: {body_str}"
    );
}

#[tokio::test]
async fn test_attest_membership_v2_requires_nullifier_hex_in_signing_input() {
    // v2 attestations whose `nullifierHex` field is missing must be
    // rejected before any network round-trip. We populate publicSignals
    // (so the publicSignals check passes) but omit nullifierHex.
    use annex_identity::zk::{fr_to_canonical_hex, topic_hash_for_v2};

    let v2_vkey = Some(load_dummy_vkey());
    let (app, _signing_key) =
        setup_app_with_known_instance("http://remote.example.com", v2_vkey).await;
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let topic = "annex:topic:test".to_string();
    let real_topic_hash = topic_hash_for_v2(&topic).unwrap();
    let real_topic_hash_hex = fr_to_canonical_hex(real_topic_hash);

    let payload = AttestationRequest {
        originating_server: "http://remote.example.com".to_string(),
        topic,
        commitment: "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        proof: serde_json::json!({}),
        participant_type: "AI_AGENT".to_string(),
        signature: hex::encode([0u8; 64]),
        protocol_version: Some("v2".to_string()),
        // Provide publicSignals so we hit the nullifier check, not the
        // missing-publicSignals check.
        public_signals: Some(vec![
            "0".to_string(),
            "1".to_string(),
            "2".to_string(),
            real_topic_hash.to_string(),
        ]),
        nullifier_hex: None,
        topic_hash_hex: Some(real_topic_hash_hex),
    };

    let mut request = Request::builder()
        .uri("/api/federation/attest-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(
        body_str.contains("v2 attestation must include nullifierHex"),
        "Expected v2 missing-nullifier error, got: {body_str}"
    );
}

#[tokio::test]
async fn test_attest_membership_v2_topic_mismatch_rejected() {
    // v2 attestation must reject proofs whose publicSignals[3] (topicHash)
    // does not match the canonical hash of payload.topic. We pass a
    // valid signature for the v2 wire format but a publicSignals[3]
    // that's clearly wrong (constant 0x...01 instead of the real hash).
    use annex_identity::zk::topic_hash_for_v2;

    let v2_vkey = Some(load_dummy_vkey());
    let (app, signing_key) =
        setup_app_with_known_instance("http://remote.example.com", v2_vkey).await;
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let topic = "annex:topic:alpha".to_string();
    let commitment = "0000000000000000000000000000000000000000000000000000000000000001".to_string();
    let participant_type = "AI_AGENT".to_string();
    // Real topicHash, but we'll lie in publicSignals[3] below.
    let real_topic_hash = topic_hash_for_v2(&topic).unwrap();
    let real_topic_hash_hex = annex_identity::zk::fr_to_canonical_hex(real_topic_hash);
    let nullifier_hex =
        "0000000000000000000000000000000000000000000000000000000000000002".to_string();

    let signing_message = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        topic, commitment, participant_type, "v2", nullifier_hex, real_topic_hash_hex
    );
    let signature = signing_key.sign(signing_message.as_bytes());
    let signature_hex = hex::encode(signature.to_bytes());

    let payload = AttestationRequest {
        originating_server: "http://remote.example.com".to_string(),
        topic,
        commitment: commitment.clone(),
        proof: serde_json::json!({}),
        participant_type,
        signature: signature_hex,
        protocol_version: Some("v2".to_string()),
        // publicSignals: [root_placeholder, commitment, nullifier, WRONG_topicHash]
        // The "remote root" comes from the local /api/federation/vrp-root
        // call — we don't know what it'll be, so we use 0 here. The
        // topic-binding check fires BEFORE the root check rejects, so
        // the test still covers the topic-binding error path.
        public_signals: Some(vec![
            "0".to_string(),
            "1".to_string(),
            "2".to_string(),
            // Use 1 (not the real topicHash) so publicSignals[3] != expected
            "1".to_string(),
        ]),
        nullifier_hex: Some(nullifier_hex),
        topic_hash_hex: Some(real_topic_hash_hex),
    };

    let mut request = Request::builder()
        .uri("/api/federation/attest-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    // ZkVerification → 400 Bad Request
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    // Either the root mismatch (publicSignals[0] != remote_root) OR the
    // topic-binding rejection. Both are correct rejections of the
    // tampered v2 envelope; the important thing is that the request did
    // NOT reach the verifier with an attacker-controlled topicHash.
    assert!(
        body_str.contains("topicHash") || body_str.contains("publicSignals[0]"),
        "Expected topic-binding or root rejection, got: {body_str}"
    );
}

#[tokio::test]
async fn test_attest_membership_v2_requires_public_signals() {
    // v2 attestation must reject requests that omit publicSignals (the
    // server cannot reconstruct them from public-only data because
    // nullifier and topicHash are prover-bound).
    let v2_vkey = Some(load_dummy_vkey());
    let (app, signing_key) =
        setup_app_with_known_instance("http://remote.example.com", v2_vkey).await;
    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let topic = "annex:topic:alpha".to_string();
    let commitment = "0000000000000000000000000000000000000000000000000000000000000001".to_string();
    let participant_type = "AI_AGENT".to_string();
    let nullifier_hex =
        "0000000000000000000000000000000000000000000000000000000000000002".to_string();
    let topic_hash_hex =
        "0000000000000000000000000000000000000000000000000000000000000003".to_string();

    let signing_message = format!(
        "{}\n{}\n{}\n{}\n{}\n{}",
        topic, commitment, participant_type, "v2", nullifier_hex, topic_hash_hex
    );
    let signature = signing_key.sign(signing_message.as_bytes());
    let signature_hex = hex::encode(signature.to_bytes());

    let payload = AttestationRequest {
        originating_server: "http://remote.example.com".to_string(),
        topic,
        commitment,
        proof: serde_json::json!({}),
        participant_type,
        signature: signature_hex,
        protocol_version: Some("v2".to_string()),
        public_signals: None, // omitted
        nullifier_hex: Some(nullifier_hex),
        topic_hash_hex: Some(topic_hash_hex),
    };

    let mut request = Request::builder()
        .uri("/api/federation/attest-membership")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&payload).unwrap()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();
    assert!(
        body_str.contains("publicSignals"),
        "Expected publicSignals required error, got: {body_str}"
    );
}
