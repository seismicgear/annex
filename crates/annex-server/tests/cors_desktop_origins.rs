use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::zk::{G1Affine, G2Affine, VerifyingKey};
use annex_server::{app, middleware, AppState};
use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::connect_info::ConnectInfo,
    http::{header, Method, Request, StatusCode},
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

fn build_test_state(cors_origins: Vec<String>) -> AppState {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers (id, slug, label, policy_json) VALUES (1, 'default', 'Default', '{}')",
            [],
        )
        .unwrap();
    }

    let tree = annex_identity::MerkleTree::new(20).unwrap();
    let vk = VerifyingKey {
        alpha_g1: G1Affine::default(),
        beta_g2: G2Affine::default(),
        gamma_g2: G2Affine::default(),
        delta_g2: G2Affine::default(),
        gamma_abc_g1: vec![G1Affine::default()],
    };

    AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(vk),
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: Arc::new(RwLock::new("http://localhost:3000".to_string())),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: middleware::RateLimiter::new(),
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
        cors_origins,
        enforce_zk_proofs: false,
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
    }
}

#[tokio::test]
async fn desktop_origins_allow_preflight_and_get() {
    let allowed_origins = [
        "tauri://localhost",
        "https://tauri.localhost",
        "http://tauri.localhost",
    ];

    let app = app(build_test_state(
        allowed_origins.iter().map(|o| o.to_string()).collect(),
    ));
    let client_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3000);

    for origin in allowed_origins {
        let mut preflight_req = Request::builder()
            .method(Method::OPTIONS)
            .uri("/health")
            .header(header::ORIGIN, origin)
            .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
            .body(Body::empty())
            .unwrap();
        preflight_req
            .extensions_mut()
            .insert(ConnectInfo(client_addr));
        let preflight_resp = app.clone().oneshot(preflight_req).await.unwrap();
        assert!(
            preflight_resp.status().is_success(),
            "expected successful preflight for {origin}, got {}",
            preflight_resp.status()
        );
        assert_eq!(
            preflight_resp
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            origin,
            "expected preflight allow origin to echo {origin}"
        );

        let mut get_req = Request::builder()
            .method(Method::GET)
            .uri("/health")
            .header(header::ORIGIN, origin)
            .body(Body::empty())
            .unwrap();
        get_req.extensions_mut().insert(ConnectInfo(client_addr));
        let get_resp = app.clone().oneshot(get_req).await.unwrap();
        assert_eq!(
            get_resp.status(),
            StatusCode::OK,
            "expected GET /health to succeed for {origin}"
        );
        assert_eq!(
            get_resp
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            origin,
            "expected GET allow origin to echo {origin}"
        );
    }
}
