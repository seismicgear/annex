mod common;

use annex_channels::{get_channel, Channel, ChannelError};
use annex_types::{AlignmentStatus, ChannelType, FederationScope};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::SocketAddr;
use tower::ServiceExt;

#[tokio::test]
async fn test_create_channel_success() {
    let (app, pool) = common::setup_test_app().await;

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
        "channel_id": "chan-new",
        "name": "New Channel",
        "channel_type": "Text",
        "topic": "A new channel",
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
        let channel = get_channel(&conn, "chan-new").unwrap();
        assert_eq!(channel.name, "New Channel");
        assert_eq!(channel.agent_min_alignment, Some(AlignmentStatus::Aligned));
    }
}

#[tokio::test]
async fn test_create_channel_forbidden() {
    let (app, pool) = common::setup_test_app().await;

    // Seed normal user (cannot moderate)
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active) VALUES (1, 'user-1', 'HUMAN', 0, 1)",
            [],
        )
        .unwrap();
    }

    let body_json = serde_json::json!({
        "channel_id": "chan-fail",
        "name": "Fail Channel",
        "channel_type": "Text",
        "topic": null,
        "vrp_topic_binding": null,
        "required_capabilities_json": null,
        "agent_min_alignment": null,
        "retention_days": null,
        "federation_scope": "Local"
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels")
        .method("POST")
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::from(body_json.to_string()))
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_list_channels() {
    let (app, pool) = common::setup_test_app().await;

    // Seed channels and user
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active) VALUES (1, 'user-1', 'HUMAN', 1)",
            [],
        )
        .unwrap();

        // Use create_channel helper directly for seeding
        // We can't use helper easily as we need to construct Params.
        // Or just raw SQL.
        // Let's use raw SQL for simplicity and avoiding dependency on annex-channels write logic if we can.
        // But using annex-channels::create_channel is safer.
        use annex_channels::create_channel;
        let params1 = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "chan-1".to_string(),
            name: "Alpha".to_string(), // A
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params1).unwrap();

        let params2 = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "chan-2".to_string(),
            name: "Beta".to_string(), // B
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params2).unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels")
        .method("GET")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let channels: Vec<Channel> = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(channels.len(), 2);
    // Sort order is by name ASC
    assert_eq!(channels[0].channel_id, "chan-1");
    assert_eq!(channels[1].channel_id, "chan-2");
}

#[tokio::test]
async fn test_get_channel() {
    let (app, pool) = common::setup_test_app().await;

    {
        let conn = pool.get().unwrap();
        // Create user with can_moderate so they can view channels.
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active) VALUES (1, 'user-1', 'HUMAN', 1, 1)",
            [],
        )
        .unwrap();

        let params = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "chan-get".to_string(),
            name: "Get Me".to_string(),
            channel_type: ChannelType::Text,
            topic: Some("Topic".to_string()),
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        annex_channels::create_channel(&conn, &params).unwrap();

        // Join the channel so the GET endpoint allows access.
        annex_channels::add_member(&conn, 1, "chan-get", "user-1").unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/chan-get")
        .method("GET")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let body_bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let channel: Channel = serde_json::from_slice(&body_bytes).unwrap();

    assert_eq!(channel.channel_id, "chan-get");
    assert_eq!(channel.topic, Some("Topic".to_string()));
}

#[tokio::test]
async fn test_delete_channel_success() {
    let (app, pool) = common::setup_test_app().await;

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active) VALUES (1, 'mod-1', 'HUMAN', 1, 1)",
            [],
        )
        .unwrap();

        let params = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "chan-del".to_string(),
            name: "To Delete".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        annex_channels::create_channel(&conn, &params).unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/chan-del")
        .method("DELETE")
        .header("X-Annex-Pseudonym", "mod-1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Verify gone
    {
        let conn = pool.get().unwrap();
        let res = get_channel(&conn, "chan-del");
        assert!(matches!(res, Err(ChannelError::NotFound(_))));
    }
}

#[tokio::test]
async fn test_delete_channel_forbidden() {
    let (app, pool) = common::setup_test_app().await;

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active) VALUES (1, 'user-1', 'HUMAN', 0, 1)",
            [],
        )
        .unwrap();

        let params = annex_channels::CreateChannelParams {
            server_id: 1,
            channel_id: "chan-safe".to_string(),
            name: "Safe".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        annex_channels::create_channel(&conn, &params).unwrap();
    }

    let addr = SocketAddr::from(([127, 0, 0, 1], 12345));
    let mut request = Request::builder()
        .uri("/api/channels/chan-safe")
        .method("DELETE")
        .header("X-Annex-Pseudonym", "user-1")
        .body(Body::empty())
        .unwrap();
    request.extensions_mut().insert(ConnectInfo(addr));

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}
