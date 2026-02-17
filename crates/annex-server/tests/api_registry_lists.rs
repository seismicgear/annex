use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::{MerkleTree, VrpRoleEntry, VrpTopic};
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt; // for oneshot

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    let vkey_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");

    // Ensure the key exists or panic with helpful message
    if !vkey_path.exists() {
        panic!(
            "ZK key not found at {:?}. Run setup scripts in zk/ directory.",
            vkey_path
        );
    }

    let vkey_json = std::fs::read_to_string(vkey_path).expect("failed to read vkey");
    let vk = annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");
    Arc::new(vk)
}

#[tokio::test]
async fn test_get_topics() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();
    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: "http://localhost:3000".to_string(),
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

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/registry/topics")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let topics: Vec<VrpTopic> = serde_json::from_slice(&body_bytes).unwrap();

    // Default seeded topics
    assert!(topics.len() >= 3);
    assert!(topics.iter().any(|t| t.topic == "annex:server:v1"));
    assert!(topics.iter().any(|t| t.topic == "annex:channel:v1"));
    assert!(topics.iter().any(|t| t.topic == "annex:federation:v1"));
}

#[tokio::test]
async fn test_get_roles() {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();
    drop(conn);

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: "http://localhost:3000".to_string(),
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

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    let mut request = Request::builder()
        .uri("/api/registry/roles")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let roles: Vec<VrpRoleEntry> = serde_json::from_slice(&body_bytes).unwrap();

    // Default seeded roles
    assert!(roles.len() >= 5);
    assert!(roles.iter().any(|r| r.label == "HUMAN" && r.role_code == 1));
    assert!(roles
        .iter()
        .any(|r| r.label == "AI_AGENT" && r.role_code == 2));
    assert!(roles
        .iter()
        .any(|r| r.label == "COLLECTIVE" && r.role_code == 3));
    assert!(roles
        .iter()
        .any(|r| r.label == "BRIDGE" && r.role_code == 4));
    assert!(roles
        .iter()
        .any(|r| r.label == "SERVICE" && r.role_code == 5));
}
