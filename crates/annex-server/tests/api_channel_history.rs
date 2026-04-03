mod common;

use annex_channels::{
    add_member, create_channel, create_message, CreateChannelParams, CreateMessageParams,
    Message as ChannelMessage,
};
use annex_types::{ChannelType, FederationScope};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use tower::ServiceExt;

#[tokio::test]
async fn test_get_history_success() {
    let (app, pool) = common::setup_test_app().await;

    {
        let conn = pool.get().unwrap();
        // Create Identity
        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-1', 'HUMAN', 1)", []).unwrap();

        // Create Channel
        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-hist".to_string(),
            name: "History Test".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();

        // Add Member
        add_member(&conn, 1, "chan-hist", "user-1").unwrap();

        // Create Messages
        // Msg 1 (oldest)
        create_message(
            &conn,
            &CreateMessageParams {
                channel_id: "chan-hist".to_string(),
                message_id: "msg-1".to_string(),
                sender_pseudonym: "user-1".to_string(),
                content: "First".to_string(),
                reply_to_message_id: None,
            },
        )
        .unwrap();

        // Sleep to ensure timestamp diff
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Msg 2
        create_message(
            &conn,
            &CreateMessageParams {
                channel_id: "chan-hist".to_string(),
                message_id: "msg-2".to_string(),
                sender_pseudonym: "user-1".to_string(),
                content: "Second".to_string(),
                reply_to_message_id: None,
            },
        )
        .unwrap();

        // Sleep to ensure timestamp diff
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Msg 3 (newest)
        create_message(
            &conn,
            &CreateMessageParams {
                channel_id: "chan-hist".to_string(),
                message_id: "msg-3".to_string(),
                sender_pseudonym: "user-1".to_string(),
                content: "Third".to_string(),
                reply_to_message_id: None,
            },
        )
        .unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // Test 1: Get All (default limit)
    let request = Request::builder()
        .uri("/api/channels/chan-hist/messages")
        .method("GET")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();

    // We need to clone the app for multiple requests because oneshot consumes it.
    // However, axum::Router implements Clone, but oneshot takes `self`.
    // So we can clone app before each request.

    let mut req1 = request;
    req1.extensions_mut().insert(ConnectInfo(addr));
    let response = app.clone().oneshot(req1).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let messages: Vec<ChannelMessage> = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].content, "Third"); // Reverse Chrono
    assert_eq!(messages[1].content, "Second");
    assert_eq!(messages[2].content, "First");

    // Test 2: Limit 1
    let mut req2 = Request::builder()
        .uri("/api/channels/chan-hist/messages?limit=1")
        .method("GET")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    req2.extensions_mut().insert(ConnectInfo(addr));

    let response = app.clone().oneshot(req2).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let messages: Vec<ChannelMessage> = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].content, "Third");

    // Test 3: Before (Paginating past the newest message)
    let newest_ts = messages[0].created_at.clone();
    // Simple encoding for space and colon, which are common in SQLite timestamps and invalid in URI path/query without encoding
    let encoded_ts = newest_ts.replace(" ", "%20").replace(":", "%3A");

    let mut req3 = Request::builder()
        .uri(format!(
            "/api/channels/chan-hist/messages?before={}",
            encoded_ts
        ))
        .method("GET")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    req3.extensions_mut().insert(ConnectInfo(addr));

    let response = app.clone().oneshot(req3).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let messages: Vec<ChannelMessage> = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(messages.len(), 2);
    assert_eq!(messages[0].content, "Second");
    assert_eq!(messages[1].content, "First");
}

#[tokio::test]
async fn test_get_history_forbidden() {
    let (app, pool) = common::setup_test_app().await;

    {
        let conn = pool.get().unwrap();
        // User 1 (Member)
        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-1', 'HUMAN', 1)", []).unwrap();
        // User 2 (Non-Member)
        conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-2', 'HUMAN', 1)", []).unwrap();

        let params = CreateChannelParams {
            server_id: 1,
            channel_id: "chan-priv".to_string(),
            name: "Private".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).unwrap();
        add_member(&conn, 1, "chan-priv", "user-1").unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));

    // Request as user-2
    let mut request = Request::builder()
        .uri("/api/channels/chan-priv/messages")
        .method("GET")
        .header("X-Annex-Pseudonym", "user-2")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
