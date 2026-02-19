//! Comprehensive tests for the WebSocket VoiceIntent handler.
//!
//! Covers every code path in the VoiceIntent arm of `api_ws.rs`:
//! - Authorization (only AI agents may use VoiceIntent)
//! - Channel membership check (allowed, denied, error)
//! - Voice profile lookup with default fallback
//! - TTS synthesis success and failure
//! - Voice client creation and audio publishing
//! - Voice session read/write lock interaction

use annex_channels::{add_member, create_channel, CreateChannelParams};
use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::voice::{VoiceModel, VoiceProfile};
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
        Err(_) => Arc::new(annex_identity::zk::generate_dummy_vkey()),
    }
}

/// Shared setup that creates an AppState with a mock TTS binary and model file.
///
/// The mock piper script reads stdin and writes predictable PCM data to stdout.
/// This allows the full VoiceIntent→TTS→VoiceClient→publish pipeline to succeed.
async fn setup_app_with_mock_tts(
    temp_dir: &tempfile::TempDir,
) -> (axum::Router, annex_db::DbPool, Arc<AppState>) {
    let pool = create_pool(":memory:", DbRuntimeSettings::default())
        .expect("failed to create in-memory pool");
    {
        let conn = pool.get().expect("failed to get connection");
        run_migrations(&conn).expect("migrations failed");
        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).expect("failed to serialize policy");
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .expect("failed to insert server");
    }

    let tree = MerkleTree::new(20).expect("failed to create merkle tree");

    // Create mock piper binary that reads stdin and writes raw bytes to stdout
    let mock_piper = temp_dir.path().join("mock_piper.sh");
    tokio::fs::write(
        &mock_piper,
        "#!/bin/sh\ncat > /dev/null\nprintf 'MOCK_PCM_AUDIO_DATA_16BIT'",
    )
    .await
    .expect("failed to write mock piper script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = tokio::fs::metadata(&mock_piper)
            .await
            .expect("mock piper metadata")
            .permissions();
        perms.set_mode(0o755);
        tokio::fs::set_permissions(&mock_piper, perms)
            .await
            .expect("failed to set mock piper permissions");
    }

    // Create a dummy model file (piper checks existence)
    let model_file = temp_dir.path().join("test_model.onnx");
    tokio::fs::write(&model_file, b"dummy model data")
        .await
        .expect("failed to write dummy model file");

    let livekit_config =
        annex_voice::LiveKitConfig::new("http://localhost:7880", "devkey", "devsecret");
    let voice_service = annex_voice::VoiceService::new(livekit_config);

    let tts_service = annex_voice::TtsService::new(temp_dir.path(), &mock_piper);

    // Register a "default" voice profile pointing to the test model
    let default_profile = VoiceProfile {
        id: "default".to_string(),
        name: "Default Test Voice".to_string(),
        model: VoiceModel::Piper,
        model_path: model_file
            .to_str()
            .expect("model path not valid UTF-8")
            .to_string(),
        config_path: None,
        speed: 1.0,
        pitch: 1.0,
        speaker_id: None,
    };
    tts_service.add_profile(default_profile).await;

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: "http://localhost:3000".to_string(),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(voice_service),
        tts_service: Arc::new(tts_service),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
    };

    (app(state.clone()), pool, Arc::new(state))
}

/// Helper: start the server, return address.
async fn start_server(app: axum::Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind listener");
    let addr = listener.local_addr().expect("failed to get local addr");
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .expect("server failed");
    });
    addr
}

/// Helper: connect WebSocket and return the socket.
async fn connect_ws(
    addr: SocketAddr,
    pseudonym: &str,
) -> tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
> {
    let url = format!("ws://{}/ws?pseudonym={}", addr, pseudonym);
    let (socket, _) = connect_async(url).await.expect("WS connect failed");
    socket
}

/// Helper: send a VoiceIntent and read back the first response.
async fn send_voice_intent_and_read(
    socket: &mut tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    channel_id: &str,
    text: &str,
) -> Value {
    let msg = json!({
        "type": "voice_intent",
        "channelId": channel_id,
        "text": text,
    });
    socket
        .send(WsMessage::Text(msg.to_string().into()))
        .await
        .expect("failed to send VoiceIntent");

    let response = tokio::time::timeout(std::time::Duration::from_secs(5), socket.next())
        .await
        .expect("timeout waiting for VoiceIntent response")
        .expect("stream ended")
        .expect("socket error");

    match response {
        WsMessage::Text(text) => serde_json::from_str(text.as_str()).expect("invalid JSON response"),
        other => panic!("expected Text message, got {:?}", other),
    }
}

// ---------------------------------------------------------------------------
// Test: Non-agent user sends VoiceIntent → rejected
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_voice_intent_non_agent_rejected() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let (app, pool, _state) = setup_app_with_mock_tts(&temp_dir).await;

    // Seed a HUMAN user and a voice channel with the human as member
    {
        let conn = pool.get().expect("pool error");
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
             VALUES (1, 'human-1', 'HUMAN', 1)",
            [],
        )
        .expect("failed to insert human identity");

        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "voice-chan".to_string(),
            name: "Voice Channel".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("failed to create channel");
        add_member(&conn, 1, "voice-chan", "human-1").expect("failed to add member");
    }

    let addr = start_server(app).await;
    let mut socket = connect_ws(addr, "human-1").await;

    let response = send_voice_intent_and_read(&mut socket, "voice-chan", "Hello").await;

    assert_eq!(
        response.get("type").and_then(|v| v.as_str()),
        Some("error"),
        "Expected error response for non-agent VoiceIntent"
    );
    let message = response
        .get("message")
        .and_then(|v| v.as_str())
        .expect("missing error message");
    assert!(
        message.contains("Only AI agents"),
        "Error should mention agent requirement, got: {}",
        message
    );
}

// ---------------------------------------------------------------------------
// Test: Agent sends VoiceIntent to channel it is not a member of → rejected
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_voice_intent_non_member_rejected() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let (app, pool, _state) = setup_app_with_mock_tts(&temp_dir).await;

    // Seed an AI agent and a voice channel, but do NOT add the agent as member
    {
        let conn = pool.get().expect("pool error");
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
             VALUES (1, 'agent-nomem', 'AI_AGENT', 1)",
            [],
        )
        .expect("failed to insert agent identity");

        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "voice-restricted".to_string(),
            name: "Restricted Voice".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("failed to create channel");
    }

    let addr = start_server(app).await;
    let mut socket = connect_ws(addr, "agent-nomem").await;

    let response =
        send_voice_intent_and_read(&mut socket, "voice-restricted", "Hello world").await;

    assert_eq!(
        response.get("type").and_then(|v| v.as_str()),
        Some("error"),
        "Expected error response for non-member"
    );
    let message = response
        .get("message")
        .and_then(|v| v.as_str())
        .expect("missing error message");
    assert!(
        message.contains("Not a member"),
        "Error should mention membership, got: {}",
        message
    );
}

// ---------------------------------------------------------------------------
// Test: Full happy path - TTS succeeds, voice client created, audio published
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_voice_intent_tts_success_full_pipeline() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let (app, pool, state) = setup_app_with_mock_tts(&temp_dir).await;

    // Seed agent and voice channel with membership
    {
        let conn = pool.get().expect("pool error");
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
             VALUES (1, 'agent-voice', 'AI_AGENT', 1)",
            [],
        )
        .expect("failed to insert agent identity");

        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "voice-happy".to_string(),
            name: "Happy Path Voice".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("failed to create channel");
        add_member(&conn, 1, "voice-happy", "agent-voice").expect("failed to add member");
    }

    let addr = start_server(app).await;
    let mut socket = connect_ws(addr, "agent-voice").await;

    // Send VoiceIntent
    let msg = json!({
        "type": "voice_intent",
        "channelId": "voice-happy",
        "text": "Hello world from TTS",
    });
    socket
        .send(WsMessage::Text(msg.to_string().into()))
        .await
        .expect("failed to send VoiceIntent");

    // The happy path does NOT send a response message back (only errors do).
    // Wait briefly to ensure no error message arrives.
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next()).await;

    match result {
        Err(_) => {
            // Timeout = no error message = success!
        }
        Ok(Some(Ok(WsMessage::Text(text)))) => {
            let v: Value = serde_json::from_str(text.as_str()).unwrap_or_default();
            if v.get("type").and_then(|t| t.as_str()) == Some("error") {
                panic!(
                    "VoiceIntent should have succeeded but got error: {}",
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown")
                );
            }
            // Non-error message is fine (could be a broadcast or other event)
        }
        Ok(Some(Ok(WsMessage::Close(_)))) => {
            panic!("Socket closed unexpectedly after VoiceIntent");
        }
        Ok(Some(Err(e))) => {
            panic!("Socket error: {}", e);
        }
        Ok(None) => {
            panic!("Stream ended unexpectedly");
        }
        _ => {}
    }

    // Verify a voice session was created for the agent
    let sessions = state
        .voice_sessions
        .read()
        .expect("voice_sessions lock poisoned");
    assert!(
        sessions.contains_key("agent-voice"),
        "Voice session should have been created for agent-voice"
    );
    let client = sessions.get("agent-voice").expect("session missing");
    assert!(client.connected, "Voice client should be connected");
    assert_eq!(client.room_name, "voice-happy");
}

// ---------------------------------------------------------------------------
// Test: TTS fails due to missing voice profile → error propagated
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_voice_intent_tts_profile_not_found() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");

    // Create an AppState with TTS service that has NO profiles registered
    let pool = create_pool(":memory:", DbRuntimeSettings::default())
        .expect("failed to create in-memory pool");
    {
        let conn = pool.get().expect("failed to get connection");
        run_migrations(&conn).expect("migrations failed");
        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).expect("failed to serialize policy");
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .expect("failed to insert server");
    }

    let tree = MerkleTree::new(20).expect("failed to create merkle tree");
    let livekit_config =
        annex_voice::LiveKitConfig::new("http://localhost:7880", "devkey", "devsecret");

    // TTS service with no profiles registered → will fail with ProfileNotFound
    let tts_service = annex_voice::TtsService::new(temp_dir.path(), "nonexistent_piper");

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: "http://localhost:3000".to_string(),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: annex_server::api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(livekit_config)),
        tts_service: Arc::new(tts_service),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
    };

    // Seed agent and channel
    {
        let conn = pool.get().expect("pool error");
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
             VALUES (1, 'agent-noprof', 'AI_AGENT', 1)",
            [],
        )
        .expect("failed to insert identity");

        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "voice-noprof".to_string(),
            name: "No Profile Voice".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("failed to create channel");
        add_member(&conn, 1, "voice-noprof", "agent-noprof").expect("failed to add member");
    }

    let router = app(state);
    let addr = start_server(router).await;
    let mut socket = connect_ws(addr, "agent-noprof").await;

    let response =
        send_voice_intent_and_read(&mut socket, "voice-noprof", "Trying to speak").await;

    assert_eq!(
        response.get("type").and_then(|v| v.as_str()),
        Some("error"),
        "Expected error when TTS profile is missing"
    );
    let message = response
        .get("message")
        .and_then(|v| v.as_str())
        .expect("missing error message");
    assert!(
        message.contains("TTS failed"),
        "Error should mention TTS failure, got: {}",
        message
    );
}

// ---------------------------------------------------------------------------
// Test: Voice profile default fallback when agent has no voice_profile_id
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_voice_intent_default_profile_fallback() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let (app, pool, state) = setup_app_with_mock_tts(&temp_dir).await;

    // Seed an agent WITHOUT a voice_profile_id in agent_registrations.
    // The VoiceIntent handler queries agent_registrations JOIN voice_profiles.
    // When no row is found, it falls back to profile_id "default".
    {
        let conn = pool.get().expect("pool error");
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
             VALUES (1, 'agent-defprof', 'AI_AGENT', 1)",
            [],
        )
        .expect("failed to insert identity");

        // Insert agent registration WITHOUT voice_profile_id
        conn.execute(
            "INSERT INTO agent_registrations (server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, last_handshake_at, active)
             VALUES (1, 'agent-defprof', 'ALIGNED', 'NO_TRANSFER', '{}', datetime('now'), 1)",
            [],
        )
        .expect("failed to insert agent registration");

        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "voice-defprof".to_string(),
            name: "Default Profile Voice".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("failed to create channel");
        add_member(&conn, 1, "voice-defprof", "agent-defprof").expect("failed to add member");
    }

    let addr = start_server(app).await;
    let mut socket = connect_ws(addr, "agent-defprof").await;

    // Send VoiceIntent - should use "default" profile and succeed
    let msg = json!({
        "type": "voice_intent",
        "channelId": "voice-defprof",
        "text": "Using default voice",
    });
    socket
        .send(WsMessage::Text(msg.to_string().into()))
        .await
        .expect("failed to send VoiceIntent");

    // Happy path: no error message expected
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), socket.next()).await;

    match result {
        Err(_) => {
            // Timeout = no error = success
        }
        Ok(Some(Ok(WsMessage::Text(text)))) => {
            let v: Value = serde_json::from_str(text.as_str()).unwrap_or_default();
            if v.get("type").and_then(|t| t.as_str()) == Some("error") {
                panic!(
                    "VoiceIntent with default profile should succeed, got error: {}",
                    v.get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("unknown")
                );
            }
        }
        _ => {}
    }

    // Verify voice session was created
    let sessions = state
        .voice_sessions
        .read()
        .expect("voice_sessions lock poisoned");
    assert!(
        sessions.contains_key("agent-defprof"),
        "Voice session should have been created with default profile fallback"
    );
}

// ---------------------------------------------------------------------------
// Test: Voice client reuse — second VoiceIntent reuses existing session
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_voice_intent_reuses_existing_voice_session() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let (app, pool, state) = setup_app_with_mock_tts(&temp_dir).await;

    // Seed agent and channel
    {
        let conn = pool.get().expect("pool error");
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
             VALUES (1, 'agent-reuse', 'AI_AGENT', 1)",
            [],
        )
        .expect("failed to insert identity");

        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "voice-reuse".to_string(),
            name: "Reuse Voice".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("failed to create channel");
        add_member(&conn, 1, "voice-reuse", "agent-reuse").expect("failed to add member");
    }

    let addr = start_server(app).await;
    let mut socket = connect_ws(addr, "agent-reuse").await;

    // First VoiceIntent: creates voice session
    let msg1 = json!({
        "type": "voice_intent",
        "channelId": "voice-reuse",
        "text": "First message",
    });
    socket
        .send(WsMessage::Text(msg1.to_string().into()))
        .await
        .expect("failed to send first VoiceIntent");

    // Wait for processing
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify session was created and capture the pointer for comparison
    let first_session_ptr = {
        let sessions = state
            .voice_sessions
            .read()
            .expect("voice_sessions lock poisoned");
        let client = sessions
            .get("agent-reuse")
            .expect("session should exist after first VoiceIntent");
        Arc::as_ptr(client)
    };

    // Second VoiceIntent: should reuse existing session (fast path via read lock)
    let msg2 = json!({
        "type": "voice_intent",
        "channelId": "voice-reuse",
        "text": "Second message",
    });
    socket
        .send(WsMessage::Text(msg2.to_string().into()))
        .await
        .expect("failed to send second VoiceIntent");

    // Wait for processing
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify same session is still in the map (pointer comparison)
    let second_session_ptr = {
        let sessions = state
            .voice_sessions
            .read()
            .expect("voice_sessions lock poisoned");
        let client = sessions
            .get("agent-reuse")
            .expect("session should still exist after second VoiceIntent");
        Arc::as_ptr(client)
    };

    assert_eq!(
        first_session_ptr, second_session_ptr,
        "Second VoiceIntent should reuse the same voice client (same Arc pointer)"
    );

    // Drain any messages to avoid hanging
    let _ = tokio::time::timeout(std::time::Duration::from_millis(100), socket.next()).await;
}

// ---------------------------------------------------------------------------
// Test: Voice session cleanup on WebSocket disconnect
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_voice_session_cleaned_up_on_disconnect() {
    let temp_dir = tempfile::tempdir().expect("failed to create temp dir");
    let (app, pool, state) = setup_app_with_mock_tts(&temp_dir).await;

    // Seed agent and channel
    {
        let conn = pool.get().expect("pool error");
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
             VALUES (1, 'agent-cleanup', 'AI_AGENT', 1)",
            [],
        )
        .expect("failed to insert identity");

        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "voice-cleanup".to_string(),
            name: "Cleanup Voice".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("failed to create channel");
        add_member(&conn, 1, "voice-cleanup", "agent-cleanup").expect("failed to add member");
    }

    let addr = start_server(app).await;
    let mut socket = connect_ws(addr, "agent-cleanup").await;

    // Create a voice session via VoiceIntent
    let msg = json!({
        "type": "voice_intent",
        "channelId": "voice-cleanup",
        "text": "Create session",
    });
    socket
        .send(WsMessage::Text(msg.to_string().into()))
        .await
        .expect("failed to send VoiceIntent");

    // Wait for processing
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    // Verify session exists
    {
        let sessions = state
            .voice_sessions
            .read()
            .expect("voice_sessions lock poisoned");
        assert!(
            sessions.contains_key("agent-cleanup"),
            "Voice session should exist before disconnect"
        );
    }

    // Close WebSocket (disconnect)
    socket
        .close(None)
        .await
        .expect("failed to close WebSocket");

    // Wait for cleanup to propagate
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // After disconnect, the voice session should be cleaned up
    let sessions = state
        .voice_sessions
        .read()
        .expect("voice_sessions lock poisoned");
    assert!(
        !sessions.contains_key("agent-cleanup"),
        "Voice session should be cleaned up after WebSocket disconnect"
    );
}
