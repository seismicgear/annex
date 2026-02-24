use annex_channels::create_channel;
use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::{ChannelType, FederationScope, ServerPolicy};
use futures_util::StreamExt;
use serde_json::Value;
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, Mutex, RwLock};
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message as WsMessage};

fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    Arc::new(annex_identity::zk::generate_dummy_vkey())
}

async fn setup_app_with_mock_stt(
    mock_stt_path: std::path::PathBuf,
) -> (axum::Router, annex_db::DbPool, Arc<AppState>) {
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
    let tts_service = annex_voice::TtsService::new("assets/voices", "assets/piper/piper", "assets/bark/bark_tts.py");
    let stt_service = annex_voice::SttService::new("dummy_model", mock_stt_path);

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        signing_key: std::sync::Arc::new(ed25519_dalek::SigningKey::generate(
            &mut rand::rngs::OsRng,
        )),
        public_url: std::sync::Arc::new(std::sync::RwLock::new("http://localhost:3000".to_string())),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(voice_service),
        tts_service: Arc::new(tts_service),
        stt_service: Arc::new(stt_service),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
        cors_origins: vec![],
        enforce_zk_proofs: false,
    };

    (app(state.clone()), pool, Arc::new(state))
}

#[tokio::test]
async fn test_agent_stt_pipeline() {
    // 1. Create mock whisper script
    let temp_dir = tempfile::tempdir().unwrap();
    let script_path = temp_dir.path().join("mock_whisper.sh");

    // The script reads stdin (ignored) and outputs "Transcribed text" to stdout
    tokio::fs::write(&script_path, "#!/bin/sh\necho -n 'Transcribed text'")
        .await
        .unwrap();

    let mut perms = tokio::fs::metadata(&script_path)
        .await
        .unwrap()
        .permissions();
    perms.set_mode(0o755);
    tokio::fs::set_permissions(&script_path, perms)
        .await
        .unwrap();

    let (app, pool, state) = setup_app_with_mock_stt(script_path).await;

    // Seed agent and voice channel
    {
        let conn = pool.get().unwrap();
        // Create agent
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'agent-stt', 'AI_AGENT', 1)",
            [],
        )
        .unwrap();

        // Agent needs a registration to pass join checks (alignment)
        conn.execute(
            "INSERT INTO agent_registrations (server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, last_handshake_at) VALUES (1, 'agent-stt', 'ALIGNED', 'NO_TRANSFER', '{}', datetime('now'))",
            [],
        ).unwrap();

        // Create voice channel
        let params = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "voice-stt".to_string(),
            name: "Voice STT".to_string(),
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

    // Start server
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

    // 1. Agent Connects via WebSocket
    let ws_url = format!("ws://{}/ws?pseudonym=agent-stt", addr);
    let (mut socket, _) = connect_async(ws_url).await.expect("Failed to connect WS");

    // 2. Agent Joins Voice Channel via API
    // We use reqwest to simulate the API call.
    // Need to authenticate via X-Annex-Pseudonym header? Or logic allows it.
    // The middleware checks X-Annex-Pseudonym.

    // Actually, join_channel_handler is protected by auth middleware.
    // We need to send the request with the header.

    let client = reqwest::Client::new();
    let join_url = format!("http://{}/api/channels/voice-stt/join", addr);
    let res = client
        .post(&join_url)
        .header("X-Annex-Pseudonym", "agent-stt")
        .send()
        .await
        .expect("Failed to send join request");

    assert_eq!(res.status(), 200);

    // 3. Verify AgentVoiceClient is created and in session map
    // Wait a bit for the async task in handler to complete
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let voice_client = {
        let sessions = state.voice_sessions.read().unwrap();
        sessions
            .get("agent-stt")
            .cloned()
            .expect("AgentVoiceClient not found in session map")
    };

    assert!(voice_client.connected);

    // 4. Trigger hearing simulation
    voice_client
        .simulate_hearing(b"audio data", "human-speaker")
        .await
        .expect("Simulation failed");

    // 5. Verify WebSocket receives transcription
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next()).await;

    match msg {
        Ok(Some(Ok(WsMessage::Text(text)))) => {
            println!("Received: {}", text);
            let v: Value = serde_json::from_str(text.as_str()).unwrap();

            assert_eq!(v.get("type").unwrap().as_str().unwrap(), "transcription");
            assert_eq!(v.get("channelId").unwrap().as_str().unwrap(), "voice-stt");
            assert_eq!(
                v.get("speakerPseudonym").unwrap().as_str().unwrap(),
                "human-speaker"
            );
            assert_eq!(v.get("text").unwrap().as_str().unwrap(), "Transcribed text");
        }
        _ => panic!("Expected transcription message, got {:?}", msg),
    }
}
