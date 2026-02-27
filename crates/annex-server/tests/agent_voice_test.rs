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

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let path1 = manifest.join("../../zk/keys/membership_vkey.json");
    let path2 = std::path::Path::new("zk/keys/membership_vkey.json");

    let vkey_json = std::fs::read_to_string(&path1).or_else(|_| std::fs::read_to_string(path2));

    match vkey_json {
        Ok(json) => {
            let vk =
                annex_identity::zk::parse_verification_key(&json).expect("failed to parse vkey");
            Arc::new(vk)
        }
        Err(_) => {
            println!("Could not find ZK keys, using dummy key");
            Arc::new(annex_identity::zk::generate_dummy_vkey())
        }
    }
}

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

    let livekit_config =
        annex_voice::LiveKitConfig::new("http://localhost:7880", "devkey", "devsecret");
    let voice_service = annex_voice::VoiceService::new(livekit_config);
    // Use dummy paths for TTS
    let tts_service = annex_voice::TtsService::new(
        "assets/voices",
        "assets/piper/piper",
        "assets/bark/bark_tts.py",
    );

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
        tts_service: Arc::new(tts_service),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
    };

    (app(state.clone()), pool, Arc::new(state))
}

#[tokio::test]
async fn test_agent_voice_intent_pipeline() {
    let (app, pool, _state) = setup_app().await;

    // Seed agent and voice channel
    {
        let conn = pool.get().unwrap();
        // Create agent
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'agent-1', 'AI_AGENT', 1)",
            [],
        )
        .unwrap();

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
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    // Connect WS
    let ws_url = format!("ws://{}/ws?pseudonym=agent-1", addr);
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

    // Wait for response (expecting error due to missing TTS models)
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next()).await;

    match msg {
        Ok(Some(Ok(WsMessage::Text(text)))) => {
            println!("Received: {}", text);
            let v: Value = serde_json::from_str(text.as_str()).unwrap();

            // Should be an error message because TTS failed
            assert_eq!(v.get("type").unwrap().as_str().unwrap(), "error");
            let message = v.get("message").unwrap().as_str().unwrap();

            // Verify it failed at TTS stage (proving pipeline integration)
            assert!(message.contains("TTS failed") || message.contains("Model file not found"));
        }
        Ok(Some(Ok(WsMessage::Close(_)))) => {
            panic!("Socket closed unexpectedly");
        }
        Ok(Some(Err(e))) => {
            panic!("Socket error: {}", e);
        }
        Ok(None) => {
            panic!("Stream ended unexpectedly");
        }
        Err(_) => {
            panic!("Timeout waiting for response");
        }
        _ => {
            panic!("Unexpected message type");
        }
    }
}
