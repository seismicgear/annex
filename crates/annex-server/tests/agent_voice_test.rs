mod common;

use annex_channels::{add_member, create_channel};
use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::{ChannelType, FederationScope, ServerPolicy};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};

async fn setup_app() -> (axum::Router, annex_db::DbPool, Arc<AppState>) {
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

    let webrtc_config =
        annex_voice::WebRtcConfig::new("http://localhost:7880", "devkey", "devsecret");
    let voice_service = annex_voice::VoiceService::new(webrtc_config);
    // Deliberately unavailable provider, independent of ambient speech models.
    // This test proves refusal and cleanup, not successful synthesis.
    let tts_service = annex_voice::TtsService::new(
        "assets/voices",
        "assets/piper/piper",
        "assets/bark/bark_tts.py",
    );

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: common::load_vkey_or_dummy(),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
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
        tts_service: Arc::new(tts_service),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        invite_base_url: "https://monolithannex.com/invite".to_string(),
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
        voice_token_secret: std::sync::Arc::new([0u8; 32]),
        federation_transport: Default::default(),
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
        shutdown: Default::default(),
        metrics: Default::default(),
    };

    (app(state.clone()), pool, Arc::new(state))
}

#[tokio::test]
async fn unavailable_agent_voice_profile_returns_an_error_without_starting_media() {
    let (app, pool, state) = setup_app().await;

    // Seed agent and voice channel
    {
        let conn = pool.get().unwrap();
        // Create agent
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active, can_voice) VALUES (1, 'agent-1', 'AI_AGENT', 1, 1)",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO agent_registrations(server_id,pseudonym_id,alignment_status,transfer_scope,capability_contract_json,reputation_score,last_handshake_at) VALUES(1,'agent-1','ALIGNED','REFLECTION_SUMMARIES_ONLY','{}',1,datetime('now'))", []).unwrap();

        // Create voice channel
        let params = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "voice-1".to_string(),
            name: "Voice 1".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();

        // Agent join
        add_member(&conn, 1, "voice-1", "agent-1").unwrap();
    }

    // Start server on random port
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    // Connect WS
    let ws_url = format!("ws://{addr}/ws?pseudonym=agent-1");
    let (mut socket, _) = connect_async(ws_url).await.expect("Failed to connect");

    // Send VoiceIntent
    let msg = json!({
        "type": "voice_intent",
        "channelId": "voice-1",
        "text": "Hello world"
    });
    socket
        .send(WsMessage::Text(msg.to_string().into()))
        .await
        .expect("Failed to send");

    let error = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match socket
                .next()
                .await
                .expect("socket ended")
                .expect("socket error")
            {
                WsMessage::Text(text) => {
                    let value: Value = serde_json::from_str(text.as_str()).unwrap();
                    if value["type"] == "error" {
                        break value["message"].as_str().unwrap().to_owned();
                    }
                }
                WsMessage::Ping(data) => socket.send(WsMessage::Pong(data)).await.unwrap(),
                WsMessage::Close(_) => panic!("socket closed without the required error"),
                _ => {}
            }
        }
    })
    .await
    .expect("unavailable speech must return a visible error");
    assert!(
        error.contains("TTS failed") && error.to_lowercase().contains("profile"),
        "{error}"
    );
    assert!(state.voice_sessions.read().unwrap().is_empty());
    assert!(state.tts_service.get_profile("default").await.is_none());
    socket.close(None).await.unwrap();
    server.abort();
}
