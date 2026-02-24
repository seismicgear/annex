//! Integration tests verifying WebSocket error handling.
//!
//! These tests validate that errors during membership checks and message
//! persistence are properly reported to the user via WebSocket error frames
//! instead of being silently swallowed.

use annex_channels::{create_channel, CreateChannelParams};
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

/// Creates a test AppState with an in-memory DB.
///
/// The returned pool has a server (id=1) and a user identity ("user-1")
/// but the user is NOT a member of any channel.
async fn setup_test_server() -> (SocketAddr, annex_db::DbPool) {
    let db_file = tempfile::NamedTempFile::new().unwrap();
    let db_path = db_file.path().to_str().unwrap().to_string();
    // Leak the tempfile so it persists for the duration of the test.
    std::mem::forget(db_file);

    let pool = annex_db::create_pool(&db_path, annex_db::DbRuntimeSettings::default()).unwrap();
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

        // Create Identity (active, but NOT a member of any channel)
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) \
             VALUES (1, 'user-1', 'HUMAN', 1)",
            [],
        )
        .unwrap();

        // Create a channel that user-1 is NOT a member of
        let chan_params = CreateChannelParams {
            server_id: 1,
            channel_id: "restricted-chan".to_string(),
            name: "Restricted".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: Some(AlignmentStatus::Aligned),
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &chan_params).unwrap();
    }

    let tree = {
        let conn = pool.get().unwrap();
        MerkleTree::restore(&conn, 20).unwrap()
    };

    let vkey_path = "zk/keys/membership_vkey.json";
    let vkey = match std::fs::read_to_string(vkey_path) {
        Ok(s) => annex_identity::zk::parse_verification_key(&s).expect("failed to parse vkey"),
        Err(_) => match std::fs::read_to_string(format!("../../{}", vkey_path)) {
            Ok(s) => annex_identity::zk::parse_verification_key(&s).expect("failed to parse vkey"),
            Err(_) => annex_identity::zk::generate_dummy_vkey(),
        },
    };

    let state = AppState {
        pool: pool.clone(),
        merkle_tree: Arc::new(Mutex::new(tree)),
        membership_vkey: Arc::new(vkey),
        server_id: 1,
        signing_key: Arc::new(ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng)),
        public_url: std::sync::Arc::new(std::sync::RwLock::new("http://localhost:3000".to_string())),
        policy: Arc::new(RwLock::new(ServerPolicy::default())),
        rate_limiter: RateLimiter::new(),
        connection_manager: api_ws::ConnectionManager::new(),
        presence_tx: tokio::sync::broadcast::channel(100).0,
        voice_service: Arc::new(annex_voice::VoiceService::new(
            annex_voice::LiveKitConfig::default(),
        )),
        tts_service: Arc::new(annex_voice::TtsService::new("voices", "piper")),
        stt_service: Arc::new(annex_voice::SttService::new("dummy", "dummy")),
        voice_sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
        observe_tx: tokio::sync::broadcast::channel(256).0,
        upload_dir: std::env::temp_dir().to_string_lossy().into_owned(),
        preview_cache: annex_server::api_link_preview::PreviewCache::new(),
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

    (addr, pool)
}

/// Subscribing to a channel where the user is NOT a member must return
/// an error message over the WebSocket instead of silently ignoring.
#[tokio::test]
async fn test_ws_subscribe_non_member_returns_error() {
    let (addr, _pool) = setup_test_server().await;

    let ws_url = format!("ws://{}/ws?pseudonym=user-1", addr);
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("failed to connect");

    // Attempt to subscribe to a channel we are NOT a member of
    let subscribe_msg = json!({
        "type": "subscribe",
        "channelId": "restricted-chan"
    });
    ws_stream
        .send(Message::Text(subscribe_msg.to_string().into()))
        .await
        .expect("failed to send subscribe");

    // Expect an error response
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), ws_stream.next())
        .await
        .expect("timeout waiting for response")
        .expect("connection closed")
        .expect("frame error");

    if let Message::Text(text) = response {
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("failed to parse response JSON");
        assert_eq!(
            parsed["type"], "error",
            "expected error message, got: {}",
            parsed
        );
        let msg = parsed["message"].as_str().expect("missing message field");
        assert!(
            msg.contains("Not a member"),
            "error message should indicate non-membership, got: {}",
            msg
        );
    } else {
        panic!("expected text message, got: {:?}", response);
    }
}

/// Sending a message to a channel where the user is NOT a member must return
/// an error message over the WebSocket instead of silently dropping the message.
#[tokio::test]
async fn test_ws_message_non_member_returns_error() {
    let (addr, _pool) = setup_test_server().await;

    let ws_url = format!("ws://{}/ws?pseudonym=user-1", addr);
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("failed to connect");

    // Attempt to send a message to a channel we are NOT a member of
    let msg = json!({
        "type": "message",
        "channelId": "restricted-chan",
        "content": "should fail",
        "replyTo": null
    });
    ws_stream
        .send(Message::Text(msg.to_string().into()))
        .await
        .expect("failed to send message");

    // Expect an error response
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), ws_stream.next())
        .await
        .expect("timeout waiting for response")
        .expect("connection closed")
        .expect("frame error");

    if let Message::Text(text) = response {
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("failed to parse response JSON");
        assert_eq!(
            parsed["type"], "error",
            "expected error message, got: {}",
            parsed
        );
        let msg = parsed["message"].as_str().expect("missing message field");
        assert!(
            msg.contains("Not a member"),
            "error message should indicate non-membership, got: {}",
            msg
        );
    } else {
        panic!("expected text message, got: {:?}", response);
    }
}

/// Connecting with an unknown pseudonym must be rejected at the HTTP upgrade level.
#[tokio::test]
async fn test_ws_unauthenticated_user_rejected() {
    let (addr, _pool) = setup_test_server().await;

    let ws_url = format!("ws://{}/ws?pseudonym=nonexistent", addr);
    let result = connect_async(ws_url).await;

    // Should fail to upgrade (HTTP 401)
    assert!(
        result.is_err(),
        "expected WebSocket upgrade to fail for unknown pseudonym"
    );
}

/// Sending malformed (non-JSON) text over the WebSocket must return an error
/// message to the client instead of silently dropping the input.
#[tokio::test]
async fn test_ws_malformed_message_returns_error() {
    let (addr, _pool) = setup_test_server().await;

    let ws_url = format!("ws://{}/ws?pseudonym=user-1", addr);
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("failed to connect");

    // Send invalid JSON
    ws_stream
        .send(Message::Text("this is not json".into()))
        .await
        .expect("failed to send malformed message");

    // Expect an error response
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), ws_stream.next())
        .await
        .expect("timeout waiting for response")
        .expect("connection closed")
        .expect("frame error");

    if let Message::Text(text) = response {
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("failed to parse response JSON");
        assert_eq!(
            parsed["type"], "error",
            "expected error message, got: {}",
            parsed
        );
        let msg = parsed["message"].as_str().expect("missing message field");
        assert!(
            msg.contains("invalid message format"),
            "error message should indicate invalid format, got: {}",
            msg
        );
    } else {
        panic!("expected text message, got: {:?}", response);
    }
}

/// Sending valid JSON that doesn't match the IncomingMessage schema must
/// return an error to the client.
#[tokio::test]
async fn test_ws_unknown_message_type_returns_error() {
    let (addr, _pool) = setup_test_server().await;

    let ws_url = format!("ws://{}/ws?pseudonym=user-1", addr);
    let (mut ws_stream, _) = connect_async(ws_url).await.expect("failed to connect");

    // Send valid JSON with unknown type
    let msg = json!({"type": "nonexistent_type", "data": 42});
    ws_stream
        .send(Message::Text(msg.to_string().into()))
        .await
        .expect("failed to send unknown type message");

    // Expect an error response because the type doesn't match IncomingMessage variants
    let response = tokio::time::timeout(std::time::Duration::from_secs(5), ws_stream.next())
        .await
        .expect("timeout waiting for response")
        .expect("connection closed")
        .expect("frame error");

    if let Message::Text(text) = response {
        let parsed: serde_json::Value =
            serde_json::from_str(&text).expect("failed to parse response JSON");
        assert_eq!(
            parsed["type"], "error",
            "expected error message for unknown type, got: {}",
            parsed
        );
        let msg = parsed["message"].as_str().expect("missing message field");
        assert!(
            msg.contains("invalid message format"),
            "error message should indicate invalid format, got: {}",
            msg
        );
    } else {
        panic!("expected text message, got: {:?}", response);
    }
}
