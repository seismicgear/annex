use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::{create_platform_identity, RoleCode};
use annex_server::{
    api::{GetCapabilitiesResponse, GetIdentityResponse},
    app,
    middleware::RateLimiter,
    AppState,
};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

#[tokio::test]
async fn test_get_identity_endpoints() {
    // 1. Setup
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();

    // Seed server
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('default', 'Default Server', '{}')",
            [],
        )
        .unwrap();
    } // Drop conn

    let tree = annex_identity::MerkleTree::new(20).unwrap();
    // Use dummy vkey since app() requires it
    let vkey_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");

    let vkey_json = std::fs::read_to_string(&vkey_path).expect("failed to read vkey");
    let vk = annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(vk),
        signing_key: Arc::new(ed25519_dalek::SigningKey::from_bytes(&[0u8; 32])),
        public_url: "http://localhost:3000".to_string(),
        server_id: 1,
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
    };
    let app = app(state);

    // 2. Insert a Platform Identity directly
    let server_id = 1;
    let pseudonym_id = "test-pseudonym-123";
    let role = RoleCode::Human;

    {
        let conn = pool.get().unwrap();
        create_platform_identity(&conn, server_id, pseudonym_id, role).unwrap();

        // Update capabilities to something non-default to verify
        conn.execute(
            "UPDATE platform_identities SET can_voice = 1, can_moderate = 1 WHERE pseudonym_id = ?1",
            [pseudonym_id],
        ).unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // 3. Test GET /api/identity/:pseudonymId
    let mut req = Request::builder()
        .uri(format!("/api/identity/{}", pseudonym_id))
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let identity: GetIdentityResponse = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(identity.pseudonym_id, pseudonym_id);
    assert_eq!(identity.participant_type, role);
    assert!(identity.active);
    assert!(identity.capabilities.can_voice);
    assert!(identity.capabilities.can_moderate);
    assert!(!identity.capabilities.can_invite); // Default

    // 4. Test GET /api/identity/:pseudonymId/capabilities
    let mut req_caps = Request::builder()
        .uri(format!("/api/identity/{}/capabilities", pseudonym_id))
        .body(Body::empty())
        .unwrap();
    req_caps.extensions_mut().insert(ConnectInfo(addr));

    let resp_caps = app.clone().oneshot(req_caps).await.unwrap();
    assert_eq!(resp_caps.status(), StatusCode::OK);

    let body_bytes_caps = axum::body::to_bytes(resp_caps.into_body(), usize::MAX)
        .await
        .unwrap();
    let caps_resp: GetCapabilitiesResponse = serde_json::from_slice(&body_bytes_caps).unwrap();

    assert!(caps_resp.capabilities.can_voice);
    assert!(caps_resp.capabilities.can_moderate);
    assert!(!caps_resp.capabilities.can_invite);

    // 5. Test Not Found
    let mut req_nf = Request::builder()
        .uri("/api/identity/non-existent")
        .body(Body::empty())
        .unwrap();
    req_nf.extensions_mut().insert(ConnectInfo(addr));

    let resp_nf = app.oneshot(req_nf).await.unwrap();
    assert_eq!(resp_nf.status(), StatusCode::NOT_FOUND);
}
