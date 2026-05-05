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
        membership_vkey_v2: None,
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: Arc::new(RwLock::new("http://localhost:3000".to_string())),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: middleware::RateLimiter::new(),
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
        cors_origins,
        enforce_zk_proofs: false,
        invite_base_url: "https://monolithannex.com/invite".to_string(),
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
    }
}

async fn assert_origin_allowed(
    app: axum::Router,
    client_addr: SocketAddr,
    origin: &str,
    context: &str,
) {
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
        "{context}: expected successful preflight for {origin}, got {}",
        preflight_resp.status()
    );
    assert_eq!(
        preflight_resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap_or_else(|| panic!(
                "{context}: preflight for {origin} missing Access-Control-Allow-Origin"
            )),
        origin,
        "{context}: expected preflight allow origin to echo {origin}"
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
        "{context}: expected GET /health to succeed for {origin}"
    );
    assert_eq!(
        get_resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .unwrap_or_else(|| panic!(
                "{context}: GET for {origin} missing Access-Control-Allow-Origin"
            )),
        origin,
        "{context}: expected GET allow origin to echo {origin}"
    );
}

async fn assert_origin_blocked(
    app: axum::Router,
    client_addr: SocketAddr,
    origin: &str,
    context: &str,
) {
    let mut get_req = Request::builder()
        .method(Method::GET)
        .uri("/health")
        .header(header::ORIGIN, origin)
        .body(Body::empty())
        .unwrap();
    get_req.extensions_mut().insert(ConnectInfo(client_addr));
    let get_resp = app.clone().oneshot(get_req).await.unwrap();
    // tower-http signals a blocked origin by omitting the
    // Access-Control-Allow-Origin header entirely.
    assert!(
        get_resp
            .headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .is_none(),
        "{context}: origin {origin} should NOT have been allowed, but server echoed ACAO"
    );
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
        assert_origin_allowed(app.clone(), client_addr, origin, "desktop origin").await;
    }
}

/// Regression test for GH-256: `cargo tauri dev` serves the UI from Vite on
/// `http://localhost:5173`, but the desktop binary only configures the
/// `tauri://localhost` family of origins. A debug build must transparently
/// accept any localhost origin so dev workflows work without hand-configuring
/// `ANNEX_CORS_ORIGINS`. Release builds (tested below) must NOT relax this.
#[cfg(debug_assertions)]
#[tokio::test]
async fn debug_build_allows_localhost_dev_origins_not_in_list() {
    let desktop_origins = [
        "tauri://localhost",
        "https://tauri.localhost",
        "http://tauri.localhost",
    ];
    let app = app(build_test_state(
        desktop_origins.iter().map(|o| o.to_string()).collect(),
    ));
    let client_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3000);

    // Vite default, Vite alternate ports (if :5173 is taken), and raw loopback.
    let dev_origins = [
        "http://localhost:5173",
        "http://localhost:5174",
        "http://127.0.0.1:5173",
        "http://[::1]:5173",
        // Also accept the bare host — some tools don't emit a port.
        "http://localhost",
    ];
    for origin in dev_origins {
        assert_origin_allowed(app.clone(), client_addr, origin, "dev localhost origin").await;
    }
}

/// Debug builds relax localhost, but they MUST NOT relax anything else.
#[cfg(debug_assertions)]
#[tokio::test]
async fn debug_build_still_blocks_non_localhost_origins() {
    let desktop_origins = [
        "tauri://localhost",
        "https://tauri.localhost",
        "http://tauri.localhost",
    ];
    let app = app(build_test_state(
        desktop_origins.iter().map(|o| o.to_string()).collect(),
    ));
    let client_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3000);

    let hostile_origins = [
        "https://evil.example.com",
        // Lookalikes that must NOT match the loopback predicate.
        "http://localhost.evil.com",
        "http://evil.localhost.example.com",
        "http://127.0.0.2",
        "http://10.0.0.1",
    ];
    for origin in hostile_origins {
        assert_origin_blocked(app.clone(), client_addr, origin, "hostile origin").await;
    }
}

/// Release builds keep the strict allowlist — localhost relaxation is compiled
/// out. Only runs under `cargo test --release`.
#[cfg(not(debug_assertions))]
#[tokio::test]
async fn release_build_does_not_relax_localhost() {
    let desktop_origins = [
        "tauri://localhost",
        "https://tauri.localhost",
        "http://tauri.localhost",
    ];
    let app = app(build_test_state(
        desktop_origins.iter().map(|o| o.to_string()).collect(),
    ));
    let client_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 3000);

    assert_origin_blocked(
        app.clone(),
        client_addr,
        "http://localhost:5173",
        "release build localhost",
    )
    .await;
}
