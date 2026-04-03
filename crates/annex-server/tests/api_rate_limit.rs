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
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

#[tokio::test]
async fn test_rate_limiting_registration() {
    // 1. Setup
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        annex_db::run_migrations(&conn).unwrap();
    }

    // Initialize Merkle Tree (in-memory for test)
    let tree = MerkleTree::new(20).unwrap();

    // Configure Policy with low rate limit
    let mut policy = ServerPolicy::default();
    policy.rate_limit.registration_limit = 2; // Allow 2 requests per minute

    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: common::load_vkey_or_dummy(),
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
        invite_base_url: "https://monolithannex.com/invite".to_string(),
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
    };

    let app = app(state);

    // 2. Execute requests
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 12345);

    for i in 1..=4 {
        let commitment = format!("{:064x}", i);
        let body_json = serde_json::json!({
            "commitmentHex": commitment,
            "roleCode": 1,
            "nodeId": 100 + i
        });

        let mut request = Request::builder()
            .uri("/api/registry/register")
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(body_json.to_string()))
            .unwrap();

        // Inject ConnectInfo manually as if extracted from connection
        request.extensions_mut().insert(ConnectInfo(addr));

        // Use app.clone() because oneshot consumes the service
        let response = app.clone().oneshot(request).await.unwrap();

        if i <= 2 {
            assert_eq!(
                response.status(),
                StatusCode::OK,
                "Request {} should succeed",
                i
            );
        } else {
            assert_eq!(
                response.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "Request {} should be rate limited",
                i
            );
            let headers = response.headers();
            assert!(headers.contains_key("retry-after"));
        }
    }
}
