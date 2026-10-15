//! Real HTTP/WebSocket regressions for agent media isolation and teardown.
//! Identity rows are fixtures; channel and voice actions use the running router.
mod common;

use annex_channels::{create_channel, CreateChannelParams};
use annex_db::{create_pool, run_migrations, DbRuntimeSettings};
use annex_identity::MerkleTree;
use annex_server::{app, AppState};
use annex_types::{ChannelType, FederationScope, ServerPolicy};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio_tungstenite::{connect_async, tungstenite::Message};

struct Harness {
    state: Arc<AppState>,
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn harness() -> Harness {
    let pool = create_pool(":memory:", DbRuntimeSettings::default()).unwrap();
    {
        let conn = pool.get().unwrap();
        run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('voice', 'Voice', ?1)",
            [serde_json::to_string(&ServerPolicy::default()).unwrap()],
        )
        .unwrap();
        for (id, kind, moderator) in [("agent", "AI_AGENT", 0), ("moderator", "HUMAN", 1)] {
            conn.execute("INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active, can_voice, can_moderate) VALUES (1, ?1, ?2, 1, 1, ?3)",
                rusqlite::params![id, kind, moderator]).unwrap();
        }
        // This fixture is a registered aligned agent. Its voice policy gates
        // must be reached without relying on the removed unregistered-agent
        // default-profile fallback. No provider synthesis is claimed here.
        conn.execute(
            "INSERT INTO agent_registrations (server_id, pseudonym_id, alignment_status, transfer_scope, capability_contract_json, last_handshake_at, active)
             VALUES (1, 'agent', 'ALIGNED', 'NO_TRANSFER', '{}', datetime('now'), 1)",
            [],
        ).unwrap();
        for (id, kind) in [
            ("voice-a", ChannelType::Voice),
            ("voice-b", ChannelType::Hybrid),
            ("text", ChannelType::Text),
        ] {
            create_channel(
                &conn,
                &CreateChannelParams {
                    server_id: 1,
                    channel_id: id.into(),
                    name: id.into(),
                    channel_type: kind,
                    topic: None,
                    vrp_topic_binding: None,
                    required_capabilities_json: None,
                    agent_min_alignment: None,
                    retention_days: None,
                    federation_scope: FederationScope::Local,
                },
            )
            .unwrap();
        }
    }
    let state = Arc::new(common::build_app_state(
        pool,
        MerkleTree::new(20).unwrap(),
        ServerPolicy::default(),
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = app((*state).clone());
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });
    Harness {
        state,
        address,
        task,
    }
}

async fn post(h: &Harness, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{}{path}", h.address))
        .header("X-Annex-Pseudonym", "agent")
        .send()
        .await
        .unwrap()
}

fn client(h: &Harness, channel: &str) -> Arc<annex_voice::AgentVoiceClient> {
    h.state
        .voice_sessions
        .read()
        .unwrap()
        .get(&("agent".into(), channel.into()))
        .unwrap()
        .clone()
}

#[tokio::test]
async fn agent_sessions_are_channel_scoped_and_leave_closes_only_the_target() {
    let h = harness().await;
    for channel in ["voice-a", "voice-b"] {
        assert_eq!(
            post(&h, &format!("/api/channels/{channel}/join"))
                .await
                .status(),
            200
        );
    }
    let a = client(&h, "voice-a");
    let b = client(&h, "voice-b");
    assert_eq!(a.room_name, "voice-a");
    assert_eq!(b.room_name, "voice-b");
    assert!(!Arc::ptr_eq(&a, &b));
    assert_eq!(post(&h, "/api/channels/voice-a/leave").await.status(), 200);
    assert!(
        !a.is_connected(),
        "even an outstanding Arc must lose publication rights"
    );
    assert!(a.publish_audio(&[0; 640]).await.is_err());
    assert!(b.is_connected());
    assert_eq!(
        h.state.voice_sessions.read().unwrap().len(),
        2,
        "publisher plus shared room transcriber"
    );
    assert_eq!(
        post(&h, "/api/channels/voice-b/voice/leave").await.status(),
        200
    );
    assert!(!b.is_connected());
    assert!(h.state.voice_sessions.read().unwrap().is_empty());
}

#[tokio::test]
async fn deleting_channel_ends_agent_media_and_removes_serving_edges() {
    let h = harness().await;
    assert_eq!(post(&h, "/api/channels/voice-a/join").await.status(), 200);
    let a = client(&h, "voice-a");
    let response = reqwest::Client::new()
        .delete(format!("http://{}/api/channels/voice-a", h.address))
        .header("X-Annex-Pseudonym", "moderator")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(!a.is_connected());
    assert!(h.state.voice_sessions.read().unwrap().is_empty());
    let conn = h.state.pool.get().unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE to_node = 'voice-a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 0,
        "deleted channels cannot remain in the agent serving graph"
    );
}

async fn voice_intent_error(h: &Harness, channel: &str) -> String {
    let (mut socket, _) = connect_async(format!("ws://{}/ws?pseudonym=agent", h.address))
        .await
        .unwrap();
    socket
        .send(Message::Text(
            json!({"type":"voice_intent", "channelId":channel, "text":"gate check"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(3), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let body: serde_json::Value = serde_json::from_str(frame.to_text().unwrap()).unwrap();
    assert_eq!(body["type"], "error");
    body["message"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn voice_intent_enforces_channel_type_before_attempting_synthesis() {
    let h = harness().await;
    assert_eq!(post(&h, "/api/channels/text/join").await.status(), 200);
    assert!(voice_intent_error(&h, "text")
        .await
        .contains("channel does not support voice"));
}

#[tokio::test]
async fn voice_intent_enforces_operator_voice_switch() {
    let h = harness().await;
    assert_eq!(post(&h, "/api/channels/voice-a/join").await.status(), 200);
    h.state.policy.write().unwrap().voice_enabled = false;
    assert!(voice_intent_error(&h, "voice-a")
        .await
        .contains("voice disabled"));
}

#[tokio::test]
async fn voice_intent_enforces_member_voice_capability() {
    let h = harness().await;
    assert_eq!(post(&h, "/api/channels/voice-a/join").await.status(), 200);
    h.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE platform_identities SET can_voice = 0 WHERE pseudonym_id = 'agent'",
            [],
        )
        .unwrap();
    assert!(voice_intent_error(&h, "voice-a")
        .await
        .contains("voice is not enabled for this identity"));
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn expect_close(socket: &mut Socket, expected_code: u16, expected_reason: &str) {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            match socket
                .next()
                .await
                .expect("close frame before EOF")
                .expect("valid close frame")
            {
                Message::Close(Some(close)) => {
                    assert_eq!(u16::from(close.code), expected_code);
                    assert_eq!(close.reason.as_str(), expected_reason);
                    break;
                }
                Message::Close(None) => panic!("server omitted the close disposition"),
                _ => {}
            }
        }
    })
    .await
    .expect("server must transmit its close frame promptly");
}

async fn frame(socket: &mut Socket, value: serde_json::Value) -> serde_json::Value {
    if value["type"] == "message" {
        socket
            .send(Message::Text(
                json!({"type":"subscribe","channelId":value["channelId"]})
                    .to_string()
                    .into(),
            ))
            .await
            .unwrap();
    }
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let received = socket.next().await.unwrap().unwrap();
            if let Message::Text(text) = received {
                return serde_json::from_str(&text).unwrap();
            }
        }
    })
    .await
    .unwrap()
}

async fn set_voice_capability(h: &Harness, enabled: bool) {
    let response = reqwest::Client::new().patch(format!("http://{}/api/admin/members/agent/capabilities", h.address))
        .header("X-Annex-Pseudonym", "moderator")
        .json(&json!({"can_voice":enabled,"can_moderate":false,"can_invite":false,"can_federate":false,"can_bridge":false}))
        .send().await.unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
}

#[tokio::test]
async fn capability_revoke_closes_media_but_retains_text_and_fresh_voice_authority() {
    let h = harness().await;
    let (mut socket, _) = connect_async(format!("ws://{}/ws?pseudonym=agent", h.address))
        .await
        .unwrap();
    // A processed frame establishes that this is the authenticated live socket.
    assert_eq!(
        frame(
            &mut socket,
            json!({"type":"voice_intent","channelId":"voice-b","text":"before joining"})
        )
        .await["type"],
        "error"
    );
    assert_eq!(post(&h, "/api/channels/voice-b/join").await.status(), 200);
    let publisher = client(&h, "voice-b");
    set_voice_capability(&h, false).await;
    let hint = tokio::time::timeout(Duration::from_secs(4), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Text(hint) = hint else {
        panic!("capability change must preserve the text socket")
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&hint).unwrap(),
        json!({"type":"identity_capabilities_changed","canVoice":false})
    );
    assert!(!publisher.is_connected());
    assert_eq!(
        h.state
            .voice_service
            .participant_count("voice-b")
            .await
            .unwrap_or(0),
        0
    );
    let denied = frame(
        &mut socket,
        json!({"type":"voice_intent","channelId":"voice-b","text":"revoked"}),
    )
    .await;
    assert!(denied["message"]
        .as_str()
        .unwrap()
        .contains("voice is not enabled for this identity"));
    let text = frame(
        &mut socket,
        json!({"type":"message","channelId":"voice-b","content":"text survives voice revocation"}),
    )
    .await;
    assert_eq!(text["type"], "message");
    assert_eq!(text["content"], "text survives voice revocation");
    set_voice_capability(&h, true).await;
    let hint = tokio::time::timeout(Duration::from_secs(4), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let Message::Text(hint) = hint else {
        panic!("capability restoration must preserve the text socket")
    };
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&hint).unwrap(),
        json!({"type":"identity_capabilities_changed","canVoice":true})
    );
    assert_eq!(
        h.state
            .voice_service
            .participant_count("voice-b")
            .await
            .unwrap_or(0),
        0,
        "restoring capability cannot silently reconnect media"
    );
    assert_eq!(post(&h, "/api/channels/voice-b/join").await.status(), 200);
    assert!(client(&h, "voice-b").is_connected());
}

#[tokio::test]
async fn replacing_socket_ends_old_media_and_old_teardown_spares_new_publisher() {
    let h = harness().await;
    let (mut old, _) = connect_async(format!("ws://{}/ws?pseudonym=agent", h.address))
        .await
        .unwrap();
    frame(
        &mut old,
        json!({"type":"voice_intent","channelId":"voice-b","text":"establish old"}),
    )
    .await;
    assert_eq!(post(&h, "/api/channels/voice-b/join").await.status(), 200);
    let previous = client(&h, "voice-b");
    let (mut replacement, _) = connect_async(format!("ws://{}/ws?pseudonym=agent", h.address))
        .await
        .unwrap();
    let confirmation = frame(
        &mut replacement,
        json!({"type":"message","channelId":"voice-b","content":"replacement socket"}),
    )
    .await;
    assert_eq!(confirmation["type"], "message");
    assert!(
        !previous.is_connected(),
        "replacement must close media owned by the old socket"
    );
    assert_eq!(post(&h, "/api/channels/voice-b/join").await.status(), 200);
    let current = client(&h, "voice-b");
    expect_close(&mut old, 4090, "identity_active_elsewhere").await;
    let confirmation = frame(
        &mut replacement,
        json!({"type":"message","channelId":"voice-b","content":"after old teardown"}),
    )
    .await;
    assert_eq!(confirmation["type"], "message");
    assert!(current.is_connected());

    // Explicitly reclaim ownership, keeping both old device sockets in scope.
    let (mut reclaimed, _) = connect_async(format!("ws://{}/ws?pseudonym=agent", h.address))
        .await
        .unwrap();
    let confirmation = frame(
        &mut reclaimed,
        json!({"type":"message","channelId":"voice-b","content":"explicit reconnect here"}),
    )
    .await;
    assert_eq!(confirmation["type"], "message");
    expect_close(&mut replacement, 4090, "identity_active_elsewhere").await;
    assert!(!current.is_connected());
    assert_eq!(post(&h, "/api/channels/voice-b/join").await.status(), 200);
    assert!(client(&h, "voice-b").is_connected());
}

#[tokio::test]
async fn revoked_session_and_shutdown_have_distinct_wire_close_codes() {
    let h = harness().await;
    let (mut revoked, _) = connect_async(format!("ws://{}/ws?pseudonym=agent", h.address))
        .await
        .unwrap();
    frame(
        &mut revoked,
        json!({"type":"voice_intent","channelId":"voice-b","text":"establish"}),
    )
    .await;
    assert_eq!(post(&h, "/api/channels/voice-b/join").await.status(), 200);
    let publisher = client(&h, "voice-b");
    h.state.connection_manager.disconnect_user("agent").await;
    expect_close(&mut revoked, 4001, "session_revoked").await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while publisher.is_connected() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    let (mut shutdown, _) = connect_async(format!("ws://{}/ws?pseudonym=moderator", h.address))
        .await
        .unwrap();
    frame(
        &mut shutdown,
        json!({"type":"voice_intent","channelId":"voice-b","text":"establish"}),
    )
    .await;
    h.state.shutdown.cancel();
    expect_close(&mut shutdown, 1001, "server_shutting_down").await;
}

#[tokio::test]
async fn refreshed_frame_authority_closes_a_revoked_token_epoch_with_4001() {
    let h = harness().await;
    let (mut socket, _) = connect_async(format!("ws://{}/ws?pseudonym=agent", h.address))
        .await
        .unwrap();
    frame(
        &mut socket,
        json!({"type":"voice_intent","channelId":"voice-b","text":"establish"}),
    )
    .await;
    h.state
        .pool
        .get()
        .unwrap()
        .execute(
            "UPDATE platform_identities SET token_epoch=token_epoch+1 WHERE pseudonym_id='agent'",
            [],
        )
        .unwrap();
    socket
        .send(Message::Text(
            json!({"type":"subscribe","channelId":"voice-b"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    expect_close(&mut socket, 4001, "session_revoked").await;
}

#[tokio::test]
async fn moderator_http_revocation_closes_an_idle_receiving_socket_and_all_agent_media() {
    let h = harness().await;
    let (mut socket, _) = connect_async(format!("ws://{}/ws?pseudonym=agent", h.address))
        .await
        .unwrap();
    frame(
        &mut socket,
        json!({"type":"voice_intent","channelId":"voice-b","text":"establish"}),
    )
    .await;
    for room in ["voice-a", "voice-b"] {
        assert_eq!(
            post(&h, &format!("/api/channels/{room}/join"))
                .await
                .status(),
            200
        );
    }
    let a = client(&h, "voice-a");
    let b = client(&h, "voice-b");
    let response = reqwest::Client::new()
        .post(format!(
            "http://{}/api/admin/members/agent/revoke-sessions",
            h.address
        ))
        .header("X-Annex-Pseudonym", "moderator")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.json::<serde_json::Value>().await.unwrap()["token_epoch"],
        1
    );
    // No new frame is sent by the revoked client to trigger cleanup.
    expect_close(&mut socket, 4001, "session_revoked").await;
    assert!(!a.is_connected());
    assert!(!b.is_connected());
    assert!(a.publish_audio(&[0; 640]).await.is_err());
    assert!(b.publish_audio(&[0; 640]).await.is_err());
    for room in ["voice-a", "voice-b"] {
        assert_eq!(
            h.state
                .voice_service
                .participant_count(room)
                .await
                .unwrap_or(0),
            0
        );
    }
}

#[tokio::test]
async fn moderator_http_revocation_also_ends_voice_created_without_a_socket() {
    let h = harness().await;
    assert_eq!(post(&h, "/api/channels/voice-a/join").await.status(), 200);
    let publisher = client(&h, "voice-a");
    let response = reqwest::Client::new()
        .post(format!(
            "http://{}/api/admin/members/agent/revoke-sessions",
            h.address
        ))
        .header("X-Annex-Pseudonym", "moderator")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(!publisher.is_connected());
    assert!(h.state.voice_sessions.read().unwrap().is_empty());
}

#[tokio::test]
async fn already_authenticated_http_request_cannot_restore_revoked_voice_or_mint_a_grant() {
    let h = harness().await;
    assert_eq!(post(&h, "/api/channels/voice-a/join").await.status(), 200);
    // Reproduce the exact boundary after middleware extracted IdentityContext,
    // before the async handler/service resumed. The real moderator HTTP action
    // advances the epoch and closes its existing publisher.
    let admitted = {
        let conn = h.state.pool.get().unwrap();
        annex_identity::get_platform_identity(&conn, 1, "agent").unwrap()
    };
    let old_publisher = client(&h, "voice-a");
    let response = reqwest::Client::new()
        .post(format!(
            "http://{}/api/admin/members/agent/revoke-sessions",
            h.address
        ))
        .header("X-Annex-Pseudonym", "moderator")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(!old_publisher.is_connected());
    let service = annex_server::services::ChannelService::new(h.state.clone());
    let headers = axum::http::HeaderMap::new();
    let error = service
        .join_channel(&admitted, &headers, "voice-a")
        .await
        .unwrap_err();
    assert_eq!(error.status_code(), axum::http::StatusCode::FORBIDDEN);
    assert!(error.to_string().contains("session was revoked"));
    let error = service
        .join_voice_channel(&admitted, &headers, "voice-a", true)
        .await
        .unwrap_err();
    assert_eq!(error.status_code(), axum::http::StatusCode::FORBIDDEN);
    assert!(error.to_string().contains("session was revoked"));
    assert!(h.state.voice_sessions.read().unwrap().is_empty());
    assert_eq!(
        h.state
            .voice_service
            .participant_count("voice-a")
            .await
            .unwrap_or(0),
        0
    );

    // A subsequently authenticated request uses the current epoch and remains
    // allowed: revocation ends sessions, it does not deactivate the identity.
    assert_eq!(post(&h, "/api/channels/voice-a/join").await.status(), 200);
    assert!(client(&h, "voice-a").is_connected());
    assert_eq!(
        post(&h, "/api/channels/voice-a/voice/join").await.status(),
        200
    );
}

#[tokio::test]
async fn operator_voice_disable_closes_existing_rooms_and_all_publishers() {
    let h = harness().await;
    for room in ["voice-a", "voice-b"] {
        assert_eq!(
            post(&h, &format!("/api/channels/{room}/join"))
                .await
                .status(),
            200
        );
    }
    let a = client(&h, "voice-a");
    let b = client(&h, "voice-b");
    let policy = ServerPolicy {
        voice_enabled: false,
        ..ServerPolicy::default()
    };
    let response = reqwest::Client::new()
        .put(format!("http://{}/api/admin/policy", h.address))
        .header("X-Annex-Pseudonym", "moderator")
        .json(&policy)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    assert!(!a.is_connected() && !b.is_connected());
    assert!(h.state.voice_sessions.read().unwrap().is_empty());
    for room in ["voice-a", "voice-b"] {
        assert_eq!(
            h.state
                .voice_service
                .participant_count(room)
                .await
                .unwrap_or(0),
            0
        );
    }
}
