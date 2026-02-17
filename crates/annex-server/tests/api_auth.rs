use annex_db::{create_pool, DbRuntimeSettings};
use annex_identity::{create_platform_identity, MerkleTree, RoleCode};
use annex_server::{
    middleware::{auth_middleware, IdentityContext, RateLimiter},
    AppState,
};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware,
    routing::get,
    Extension, Router,
};
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    let vkey_path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../zk/keys/membership_vkey.json");

    if !vkey_path.exists() {
        // If keys don't exist, we can't run this test easily without them unless we mock.
        // But since this test doesn't use the vkey, we can try to construct a default one
        // if annex-identity exposed a way, but it doesn't easily.
        // We will panic with a helpful message.
        panic!(
            "ZK keys not found at {:?}. Run `cargo test -p annex-identity` to generate them.",
            vkey_path
        );
    }

    let vkey_json = std::fs::read_to_string(vkey_path).expect("failed to read vkey");
    let vk = annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");
    Arc::new(vk)
}

#[tokio::test]
async fn test_auth_middleware_flow() {
    // 1. Setup DB
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    let conn = pool.get().unwrap();
    annex_db::run_migrations(&conn).unwrap();

    // 2. Seed Server
    conn.execute(
        "INSERT INTO servers (slug, label, policy_json) VALUES (?1, ?2, ?3)",
        rusqlite::params!["test-server", "Test Server", "{}"],
    )
    .unwrap();

    // 3. Seed Identity
    create_platform_identity(&conn, 1, "valid-pseudo", RoleCode::Human).unwrap();
    drop(conn);

    // 4. Setup AppState
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

    // 5. Setup Router with middleware
    // We add a route that returns the identity found in extensions
    let app = Router::new()
        .route(
            "/protected",
            get(
                |Extension(identity): Extension<IdentityContext>| async move {
                    format!("Hello {}", identity.0.pseudonym_id)
                },
            ),
        )
        .layer(middleware::from_fn(auth_middleware))
        .layer(Extension(Arc::new(state.clone())));

    // Test 1: No header
    let req = Request::builder()
        .uri("/protected")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Test 2: Invalid header
    let req = Request::builder()
        .uri("/protected")
        .header("X-Annex-Pseudonym", "invalid-pseudo")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    // Test 3: Valid header (X-Annex-Pseudonym)
    let req = Request::builder()
        .uri("/protected")
        .header("X-Annex-Pseudonym", "valid-pseudo")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body_bytes, "Hello valid-pseudo");

    // Test 4: Valid header (Authorization Bearer)
    let req = Request::builder()
        .uri("/protected")
        .header("Authorization", "Bearer valid-pseudo")
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(req).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body_bytes, "Hello valid-pseudo");
}
