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
async fn test_ws_lifecycle() {
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

        // Create Identity
        let pseudo = "user-1".to_string();
        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, ?1, 'HUMAN', 1)", [&pseudo]).unwrap();

        // Create Channel
        let chan_params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-1".to_string(),
            name: "General".to_string(),
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
        add_member(&conn, 1, "chan-1", "user-1").unwrap();
    }

    // 2. Setup AppState
    let tree = {
        let conn = pool.get().unwrap();
        MerkleTree::restore(&conn, 20).unwrap()
    };

    // Load the real vkey if available, otherwise fall back to the dummy
    // (matches tests/common/mod.rs::load_vkey_or_dummy). Test sets
    // `enforce_zk_proofs = false`, so the dummy is acceptable.
    let vkey_path = "zk/keys/membership_vkey.json";
    let vkey = match std::fs::read_to_string(vkey_path)
        .or_else(|_| std::fs::read_to_string(format!("../../{vkey_path}")))
    {
        Ok(s) => annex_identity::zk::parse_verification_key(&s).expect("failed to parse vkey"),
        Err(_) => annex_identity::zk::generate_dummy_vkey(),
    };

    let signing_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
    let at_rest_cipher =
        annex_server::at_rest::MessageCipher::from_signing_key(&signing_key.to_bytes());
    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(vkey),
        membership_vkey_v2: None,
        channel_eligibility_vkey: None,
        link_pseudonyms_vkey: None,
        federation_attestation_vkey: None,
        server_id: 1,
        signing_key: std::sync::Arc::new(signing_key),
        public_url: std::sync::Arc::new(std::sync::RwLock::new(
            "http://localhost:3000".to_string(),
        )),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: api_ws::ConnectionManager::new(),
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
        cors_origins: vec![],
        enforce_zk_proofs: false,
        invite_base_url: "https://monolithannex.com/invite".to_string(),
        ws_token_secret: std::sync::Arc::new([0u8; 32]),
        voice_token_secret: std::sync::Arc::new([0u8; 32]),
        federation_config: annex_server::config::FederationConfig::default(),
        storage_config: annex_server::config::StorageConfig::default(),
        storage_health: std::sync::Arc::new(annex_server::storage_health::StorageHealth::new()),
        trusted_proxy_depth: 0,
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

    // 4. Connect WS
    let ws_url = format!("ws://{addr}/ws?pseudonym=user-1");
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("failed to connect");

    // 5. Subscribe
    let subscribe_msg = json!({
        "type": "subscribe",
        "channelId": "chan-1"
    });
    ws_stream
        .send(Message::Text(subscribe_msg.to_string().into()))
        .await
        .expect("failed to send subscribe");

    // Wait a bit for subscription to process
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // 6. Send Message
    let content = "Hello WebSocket";
    let msg = json!({
        "type": "message",
        "channelId": "chan-1",
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
            // Check if it's the message we sent (echoed back due to broadcast)
            if received["type"] == "message" {
                // With serde(tag="type"), fields are flattened at top level for newtype variant holding struct
                // OR it might be wrapped if it can't flatten?
                // Let's assume flattening.
                if !received["content"].is_null() {
                    assert_eq!(received["content"], content);
                    assert_eq!(received["senderPseudonym"], "user-1");
                } else {
                    // Maybe it IS wrapped?
                    // If it's wrapped in "message" key despite my assumption:
                    if !received["message"].is_null() {
                        assert_eq!(received["message"]["content"], content);
                        assert_eq!(received["message"]["senderPseudonym"], "user-1");
                    } else {
                        // Fallback to error
                        panic!("Missing content or message field in: {received}");
                    }
                }
            } else {
                panic!("unexpected message type: {}", received["type"]);
            }
        } else {
            panic!("expected text message");
        }
    } else {
        panic!("connection closed or no message");
    }

    // 8. Verify DB
    {
        let conn = pool.get().unwrap();
        let msgs = list_messages(&conn, 1, "chan-1", None, None).unwrap();
        assert_eq!(msgs.len(), 1);
        // Stored content is encrypted at rest; decrypt to compare plaintext.
        assert_ne!(
            msgs[0].content, content,
            "content should be encrypted at rest"
        );
        assert_eq!(at_rest_cipher.decrypt(&msgs[0].content), content);
    }
}
