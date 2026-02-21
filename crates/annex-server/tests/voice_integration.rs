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
        public_url: "http://localhost:3000".to_string(),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(voice_service),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
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
