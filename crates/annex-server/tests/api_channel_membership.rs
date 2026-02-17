use annex_channels::{create_channel, is_member, list_messages, CreateChannelParams};
use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, middleware::RateLimiter, AppState};
use annex_types::{AlignmentStatus, ChannelType, FederationScope, ServerPolicy};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use ed25519_dalek::SigningKey;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};
use tower::ServiceExt;

// Helper to load verification key
fn load_vkey() -> Arc<annex_identity::zk::VerifyingKey<annex_identity::zk::Bn254>> {
    let manifest = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // If running from crates/annex-server
    let path1 = manifest.join("../../zk/keys/membership_vkey.json");
    // If running from root
    let path2 = std::path::Path::new("zk/keys/membership_vkey.json");

    let vkey_json = std::fs::read_to_string(&path1)
        .or_else(|_| std::fs::read_to_string(path2))
        .unwrap_or_else(|_| panic!("failed to read vkey from {:?} or {:?}", path1, path2));

    let vk = annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");
    Arc::new(vk)
}

async fn setup_app() -> (axum::Router, annex_db::DbPool) {
    let db_id = uuid::Uuid::new_v4();
    let db_path = format!("file:memdb{}?mode=memory&cache=shared", db_id);
    let pool = create_pool(&db_path, DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        // Create server
        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test', 'Test', ?1)",
            [policy_json],
        )
        .unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        signing_key: Arc::new(SigningKey::from_bytes(&[0u8; 32])),
        public_url: "http://localhost:3000".to_string(),
        server_id: 1,
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

    (app(state), pool)
}

#[tokio::test]
async fn test_join_channel_success() {
    let (app, pool) = setup_app().await;

    // Seed data
    {
        let conn = pool.get().unwrap();
        // Create Identity
        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-1', 'HUMAN', 1)", []).unwrap();
        // Create Channel
        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-1".to_string(),
            name: "General".to_string(),
            channel_type: ChannelType::Text,
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
        .uri("/api/channels/chan-1/join")
        .method("POST")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Verify DB
    {
        let conn = pool.get().unwrap();
        assert!(is_member(&conn, "chan-1", "user-1").unwrap());
    }
}

#[tokio::test]
async fn test_join_channel_missing_capabilities() {
    let (app, pool) = setup_app().await;

    {
        let conn = pool.get().unwrap();
        // User without voice
        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_voice, active) VALUES (1, 'user-nov', 'HUMAN', 0, 1)", []).unwrap();

        // Channel requires voice
        let caps = serde_json::to_string(&vec!["can_voice"]).unwrap();
        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-voice".to_string(),
            name: "Voice".to_string(),
            channel_type: ChannelType::Voice,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: Some(caps),
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/chan-voice/join")
        .method("POST")
        .header("X-Annex-Pseudonym", "user-nov")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_join_channel_agent_misaligned() {
    let (app, pool) = setup_app().await;

    {
        let conn = pool.get().unwrap();
        // Agent identity
        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'agent-bad', 'AI_AGENT', 1)", []).unwrap();

        // Agent registration (Conflict)
        // Table: agent_registrations
        // Columns: server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, last_handshake_at
        conn.execute(
            "INSERT INTO agent_registrations (server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, last_handshake_at)
             VALUES (1, 'agent-bad', '\"Conflict\"', '\"NoTransfer\"', '{}', datetime('now'))",
            []
        ).unwrap();

        // Channel requires Aligned
        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-aligned".to_string(),
            name: "Aligned Only".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: Some(AlignmentStatus::Aligned),
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/chan-aligned/join")
        .method("POST")
        .header("X-Annex-Pseudonym", "agent-bad")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_leave_channel() {
    let (app, pool) = setup_app().await;

    {
        let conn = pool.get().unwrap();
        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-1', 'HUMAN', 1)", []).unwrap();
        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-1".to_string(),
            name: "General".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();
        // Add member manually to test leave
        conn.execute("INSERT INTO channel_members (server_id, channel_id, pseudonym_id) VALUES (1, 'chan-1', 'user-1')", []).unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/chan-1/leave")
        .method("POST")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    {
        let conn = pool.get().unwrap();
        assert!(!is_member(&conn, "chan-1", "user-1").unwrap());
    }
}

#[tokio::test]
async fn test_ws_subscription_enforcement() {
    // 1. Setup (full server spawn needed for WS)
    let db_id = uuid::Uuid::new_v4();
    let db_path = format!("file:memdb{}?mode=memory&cache=shared", db_id);
    let pool = create_pool(&db_path, DbRuntimeSettings::default()).unwrap();
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

        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-ws', 'HUMAN', 1)", []).unwrap();

        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-ws".to_string(),
            name: "WS Test".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        signing_key: Arc::new(SigningKey::from_bytes(&[0u8; 32])),
        public_url: "http://localhost:3000".to_string(),
        server_id: 1,
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

    let app = app(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    // 2. Connect
    let ws_url = format!("ws://{}/ws?pseudonym=user-ws", addr);
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("failed to connect");

    // 3. Try Subscribe (Not a member)
    let subscribe_msg = json!({
        "type": "subscribe",
        "channelId": "chan-ws"
    });
    ws_stream
        .send(Message::Text(subscribe_msg.to_string().into()))
        .await
        .unwrap();

    // 4. Expect Error Message
    if let Some(Ok(msg)) = ws_stream.next().await {
        if let Message::Text(text) = msg {
            let received: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(received["type"], "error");
            assert!(received["message"]
                .as_str()
                .unwrap()
                .contains("Not a member"));
        } else {
            panic!("expected text message");
        }
    } else {
        panic!("connection closed or no message");
    }

    // 5. Join via REST (or manually insert to DB since we have pool access)
    {
        let conn = pool.get().unwrap();
        conn.execute("INSERT INTO channel_members (server_id, channel_id, pseudonym_id) VALUES (1, 'chan-ws', 'user-ws')", []).unwrap();
    }

    // 6. Try Subscribe Again
    ws_stream
        .send(Message::Text(subscribe_msg.to_string().into()))
        .await
        .unwrap();

    // Send a message to verify subscription worked (broadcast back)
    let content = "Hello";
    let msg = json!({
        "type": "message",
        "channelId": "chan-ws",
        "content": content,
        "replyTo": null
    });
    ws_stream
        .send(Message::Text(msg.to_string().into()))
        .await
        .unwrap();

    // 7. Receive Broadcast
    // We expect the message back.
    if let Some(Ok(msg)) = ws_stream.next().await {
        if let Message::Text(text) = msg {
            let received: serde_json::Value = serde_json::from_str(&text).unwrap();
            // Check if it's the message we sent (echoed back due to broadcast)
            if received["type"] == "message" {
                assert_eq!(received["content"], content);
            } else {
                panic!("unexpected message type: {}", received["type"]);
            }
        } else {
            panic!("expected text message");
        }
    } else {
        panic!("connection closed or no message");
    }
}

#[tokio::test]
async fn test_ws_message_enforcement() {
    // 1. Setup
    let db_id = uuid::Uuid::new_v4();
    let db_path = format!("file:memdb{}?mode=memory&cache=shared", db_id);
    let pool = create_pool(&db_path, DbRuntimeSettings::default()).unwrap();
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

        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-bad', 'HUMAN', 1)", []).unwrap();

        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-bad".to_string(),
            name: "Bad Channel".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();
    }

    let tree = MerkleTree::new(20).unwrap();
    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: load_vkey(),
        signing_key: Arc::new(SigningKey::from_bytes(&[0u8; 32])),
        public_url: "http://localhost:3000".to_string(),
        server_id: 1,
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

    let app = app(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    // 2. Connect
    let ws_url = format!("ws://{}/ws?pseudonym=user-bad", addr);
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("failed to connect");

    // 3. Try Send Message (Not a member, and NOT subscribed)
    // Even if we don't subscribe, sending should fail.
    let msg = json!({
        "type": "message",
        "channelId": "chan-bad",
        "content": "Illegal message",
        "replyTo": null
    });
    ws_stream
        .send(Message::Text(msg.to_string().into()))
        .await
        .unwrap();

    // 4. Expect Error Message
    if let Some(Ok(msg)) = ws_stream.next().await {
        if let Message::Text(text) = msg {
            let received: serde_json::Value = serde_json::from_str(&text).unwrap();
            assert_eq!(received["type"], "error");
            assert!(received["message"]
                .as_str()
                .unwrap()
                .contains("Not a member"));
        } else {
            panic!("expected text message");
        }
    } else {
        panic!("connection closed or no message");
    }

    // 5. Verify Not Persisted
    {
        let conn = pool.get().unwrap();
        let messages = list_messages(&conn, "chan-bad", None, None).unwrap();
        assert_eq!(messages.len(), 0);
    }
}
