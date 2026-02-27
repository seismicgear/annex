use annex_channels::{add_member, create_channel};
use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::{ChannelType, FederationScope, ServerPolicy};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use serde_json::Value;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tower::ServiceExt;

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    let vk = annex_identity::zk::generate_dummy_vkey();
    Arc::new(vk)
}

async fn setup_app() -> (axum::Router, annex_db::DbPool) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();

    // Configure VoiceService with dummy credentials
    // This allows token generation to succeed (local operation)
    // create_room will fail (network error) but is swallowed by handler
    let livekit_config =
        annex_voice::LiveKitConfig::new("http://localhost:7880", "devkey", "devsecret");
    let voice_service = annex_voice::VoiceService::new(livekit_config);

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
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
        voice_service: Arc::new(voice_service),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
    };

    (app(state), pool)
}

#[tokio::test]
async fn test_create_voice_channel() {
    let (app, pool) = setup_app().await;

    // Seed moderator
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active) VALUES (1, 'mod-1', 'HUMAN', 1, 1)",
            [],
        )
        .unwrap();
    }

    let body_json = serde_json::json!({
        "channel_id": "voice-1",
        "name": "General Voice",
        "channel_type": "Voice",
        "topic": "Chat",
        "vrp_topic_binding": null,
        "required_capabilities_json": null,
        "agent_min_alignment": "Aligned",
        "retention_days": 30,
        "federation_scope": "Local"
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels")
        .method("POST")
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", "mod-1")
        .body(Body::from(body_json.to_string()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Verify DB
    {
        let conn = pool.get().unwrap();
        let channel = annex_channels::get_channel(&conn, "voice-1").unwrap();
        assert_eq!(channel.channel_type, ChannelType::Voice);
    }
}

#[tokio::test]
async fn test_join_voice_channel_success() {
    let (app, pool) = setup_app().await;

    // Seed user and voice channel
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-1', 'HUMAN', 1)",
            [],
        )
        .unwrap();

        let params = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "voice-join".to_string(),
            name: "Voice Join".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();

        // User must be a member to join voice
        add_member(&conn, 1, "voice-join", "user-1").unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/voice-join/voice/join")
        .method("POST")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body_bytes).unwrap();

    assert!(body.get("token").is_some());
    assert_eq!(
        body.get("url").unwrap().as_str().unwrap(),
        "http://localhost:7880"
    );

    // ICE servers should be present in the join response.
    let ice_servers = body
        .get("ice_servers")
        .expect("response must include ice_servers");
    assert!(ice_servers.is_array(), "ice_servers must be an array");
    let ice_arr = ice_servers.as_array().unwrap();
    assert!(
        !ice_arr.is_empty(),
        "default config should include STUN servers"
    );
    assert!(
        ice_arr[0].get("urls").is_some(),
        "each ICE server must have urls"
    );
}

#[tokio::test]
async fn test_join_voice_channel_forbidden_not_member() {
    let (app, pool) = setup_app().await;

    // Seed user and voice channel, but user is NOT a member
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-1', 'HUMAN', 1)",
            [],
        )
        .unwrap();

        let params = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "voice-secret".to_string(),
            name: "Secret Voice".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/voice-secret/voice/join")
        .method("POST")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_join_voice_channel_bad_request_wrong_type() {
    let (app, pool) = setup_app().await;

    // Seed user and TEXT channel
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-1', 'HUMAN', 1)",
            [],
        )
        .unwrap();

        let params = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "text-chan".to_string(),
            name: "Text Only".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();

        add_member(&conn, 1, "text-chan", "user-1").unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/text-chan/voice/join")
        .method("POST")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_leave_voice_channel_success() {
    let (app, pool) = setup_app().await;

    // Seed user and voice channel
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-1', 'HUMAN', 1)",
            [],
        )
        .unwrap();

        let params = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "voice-leave".to_string(),
            name: "Voice Leave".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();

        add_member(&conn, 1, "voice-leave", "user-1").unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/voice-leave/voice/leave")
        .method("POST")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(body.get("status").unwrap().as_str().unwrap(), "left");
}

/// Setup an app with voice DISABLED (empty LiveKit config).
async fn setup_app_voice_disabled() -> axum::Router {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();

    // LiveKit config with empty URL — voice is disabled
    let livekit_config = annex_voice::LiveKitConfig::new("", "", "");
    let voice_service = annex_voice::VoiceService::new(livekit_config);

    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
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
        voice_service: Arc::new(voice_service),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
    };

    app(state)
}

#[tokio::test]
async fn test_health_includes_voice_enabled_true() {
    let (app, _pool) = setup_app().await;

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(body["status"].as_str().unwrap(), "ok");
    assert!(body["voice_enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_health_includes_voice_enabled_false() {
    let app = setup_app_voice_disabled().await;

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/health")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(body["status"].as_str().unwrap(), "ok");
    assert!(!body["voice_enabled"].as_bool().unwrap());
}

#[tokio::test]
async fn test_voice_config_status_disabled() {
    let app = setup_app_voice_disabled().await;

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/voice/config-status")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body_bytes).unwrap();

    assert!(!body["voice_enabled"].as_bool().unwrap());
    assert!(!body["has_public_url"].as_bool().unwrap());
    assert!(
        body["setup_hint"]
            .as_str()
            .unwrap()
            .contains("not configured"),
        "setup_hint should mention LiveKit is not configured"
    );
}

#[tokio::test]
async fn test_voice_config_status_enabled() {
    let (app, _pool) = setup_app().await;

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/voice/config-status")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&body_bytes).unwrap();

    assert!(body["voice_enabled"].as_bool().unwrap());
    assert!(
        body["setup_hint"].as_str().unwrap().contains("ready"),
        "setup_hint should indicate voice is ready"
    );
}

#[tokio::test]
async fn test_voice_join_not_configured_returns_structured_error() {
    // Build a voice-disabled app with user and voice channel seeded
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-1', 'HUMAN', 1)",
            [],
        )
        .unwrap();

        let params = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "voice-test".to_string(),
            name: "Voice Test".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();
        add_member(&conn, 1, "voice-test", "user-1").unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();
    // Voice disabled: empty URL prevents voice join.
    let livekit_config = annex_voice::LiveKitConfig::new("", "", "");
    let voice_service = annex_voice::VoiceService::new(livekit_config);

    let state = AppState {
        pool,
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
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
        voice_service: Arc::new(voice_service),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper", "bark")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
    };

    let router = app(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/voice-test/voice/join")
        .method("POST")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = router.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let body_str = String::from_utf8(body_bytes.to_vec()).unwrap();

    // The error body should be parseable JSON with a structured error code
    let body: Value = serde_json::from_str(&body_str).unwrap();
    assert_eq!(
        body["error"].as_str().unwrap(),
        "voice_not_configured",
        "error should have a structured error code"
    );
    assert!(
        body["message"].as_str().unwrap().contains("not configured"),
        "message should mention voice is not configured"
    );
    assert!(
        body["setup_hint"].is_string(),
        "setup_hint should be present"
    );
}
