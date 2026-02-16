use annex_channels::{add_member, create_channel, list_messages, CreateChannelParams};
use annex_db::run_migrations;
use annex_identity::MerkleTree;
use annex_server::middleware::RateLimiter;
use annex_server::{api_ws, app, AppState};
use annex_types::{AlignmentStatus, ChannelType, FederationScope, ServerPolicy};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, RwLock};
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

#[tokio::test]
async fn test_agent_websocket_behavior() {
    // 1. Setup DB
    let pool = annex_db::create_pool(":memory:", annex_db::DbRuntimeSettings::default()).unwrap();
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

        // Create Identity for Agent
        let pseudo = "agent-007".to_string();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, ?1, 'AI_AGENT', 1)",
            [&pseudo]
        ).unwrap();

        // Create Channel
        let chan_params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-agent".to_string(),
            name: "Agent Channel".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: Some(AlignmentStatus::Aligned),
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &chan_params).unwrap();

        // Add Member
        add_member(&conn, 1, "chan-agent", "agent-007").unwrap();
    }

    // 2. Setup AppState
    let tree = {
        let conn = pool.get().unwrap();
        MerkleTree::restore(&conn, 20).unwrap()
    };

    // Load real vkey from repo root or fallback
    let vkey_path = "zk/keys/membership_vkey.json";
    let vkey_json = match std::fs::read_to_string(vkey_path) {
        Ok(s) => s,
        Err(_) => {
            std::fs::read_to_string(format!("../../{}", vkey_path)).expect("failed to read vkey")
        }
    };

    let vkey =
        annex_identity::zk::parse_verification_key(&vkey_json).expect("failed to parse vkey");

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(vkey),
        server_id: 1,
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
    };

    // 3. Start Server
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

    // 4. Connect WS as Agent
    let ws_url = format!("ws://{}/ws?pseudonym=agent-007", addr);
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("failed to connect");

    // 5. Subscribe
    let subscribe_msg = json!({
        "type": "subscribe",
        "channelId": "chan-agent"
    });
    ws_stream
        .send(Message::Text(subscribe_msg.to_string().into()))
        .await
        .expect("failed to send subscribe");

    // Wait a bit for subscription to process
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 6. Send Message
    let content = "Hello from agent";
    let msg = json!({
        "type": "message",
        "channelId": "chan-agent",
        "content": content,
        "replyTo": null
    });
    ws_stream
        .send(Message::Text(msg.to_string().into()))
        .await
        .expect("failed to send message");

    // 7. Receive Broadcast
    // We expect the message back.
    if let Some(Ok(msg)) = ws_stream.next().await {
        if let Message::Text(text) = msg {
            let received: serde_json::Value =
                serde_json::from_str(&text).expect("failed to parse json");

            // Check type and flattened fields
            assert_eq!(received["type"], "message");
            assert_eq!(received["content"], content);
            assert_eq!(received["sender_pseudonym"], "agent-007");
        } else {
            panic!("expected text message");
        }
    } else {
        panic!("connection closed or no message");
    }

    // 8. Verify DB
    {
        let conn = pool.get().unwrap();
        let msgs = list_messages(&conn, "chan-agent", None, None).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].content, content);
        assert_eq!(msgs[0].sender_pseudonym, "agent-007");
    }
}
