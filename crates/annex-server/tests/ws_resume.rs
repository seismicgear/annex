//! Reconnect: `IncomingMessage::Resume` and what it is allowed to claim.
//!
//! The client sends `resume` for every subscribed channel on every reconnect
//! (`client/src/lib/ws.ts`), naming the last message id it saw. Three very
//! different situations used to produce the identical reply — an empty replay
//! and `Resumed { missedCount: 0 }` — and the client's handler for that frame
//! is a no-op, so all three ended in silence:
//!
//!   * the client really was up to date;
//!   * the client is no longer a member of the channel;
//!   * the id it named no longer resolves.
//!
//! The third is the damaging one, and it is routine rather than exceptional:
//! `annex_channels::retention` hard-DELETEs messages past `expires_at`, so on
//! any channel with `retention_days` the id a client holds stops existing on a
//! schedule. A client offline across a purge reconnected, was told it had
//! missed nothing, and kept a timeline with a hole in it that nothing would
//! ever fill.
//!
//! These tests drive a real WebSocket against a real server, because that is
//! the only place the difference is observable — the service layer never sees
//! the frame.

use annex_channels::{add_member, create_channel, CreateChannelParams};
use annex_identity::MerkleTree;
use annex_server::{app, AppState};
use annex_types::{ChannelType, FederationScope, ServerPolicy};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use std::net::SocketAddr;
use tokio::net::TcpListener;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

mod common;

const USER: &str = "psn-resumer";
const MINE: &str = "chan-mine";
const THEIRS: &str = "chan-theirs";
const CLOSED: &str = "chan-not-a-member";

/// A server on a real port, with:
///   * `chan-mine`   — USER is a member, holds three messages;
///   * `chan-theirs` — USER is a member, holds one message (a foreign cursor);
///   * `chan-not-a-member` — USER is not a member.
async fn setup() -> (SocketAddr, annex_db::DbPool) {
    let (_router, pool) = common::setup_test_app().await;

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT OR IGNORE INTO platform_identities \
             (server_id, pseudonym_id, participant_type, active) VALUES (1, ?1, 'HUMAN', 1)",
            [USER],
        )
        .unwrap();

        for (cid, member) in [(MINE, true), (THEIRS, true), (CLOSED, false)] {
            create_channel(
                &conn,
                &CreateChannelParams {
                    server_id: 1,
                    channel_id: cid.to_string(),
                    name: cid.to_string(),
                    channel_type: ChannelType::Text,
                    topic: None,
                    vrp_topic_binding: None,
                    required_capabilities_json: None,
                    agent_min_alignment: None,
                    retention_days: None,
                    federation_scope: FederationScope::Local,
                },
            )
            .unwrap();
            if member {
                add_member(&conn, 1, cid, USER).unwrap();
            }
        }
    }

    let tree = {
        let conn = pool.get().unwrap();
        MerkleTree::restore(&conn, 20).unwrap()
    };
    let state: AppState = common::build_app_state(pool.clone(), tree, ServerPolicy::default());

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app(state);
    tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await
        .unwrap();
    });

    (addr, pool)
}

/// Insert a message directly, with an explicit `created_at` so ordering is
/// deterministic rather than dependent on how fast the test runs.
fn insert_message(
    pool: &annex_db::DbPool,
    channel_id: &str,
    message_id: &str,
    content: &str,
    created_at: &str,
) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO messages \
         (server_id, channel_id, message_id, sender_pseudonym, content, created_at) \
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![channel_id, message_id, USER, content, created_at],
    )
    .unwrap();
}

/// Send one `resume` frame and collect every frame that comes back until the
/// `resumed` ack (or an `error`, which ends the exchange just as finally).
async fn resume(
    addr: SocketAddr,
    channel_id: &str,
    last_message_id: &str,
) -> (Vec<serde_json::Value>, serde_json::Value) {
    let (mut ws, _) = connect_async(format!("ws://{addr}/ws?pseudonym={USER}"))
        .await
        .expect("connect");
    ws.send(Message::Text(
        json!({
            "type": "resume",
            "channelId": channel_id,
            "lastMessageId": last_message_id,
        })
        .to_string()
        .into(),
    ))
    .await
    .expect("send resume");

    let mut replayed = Vec::new();
    loop {
        let frame = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for a resume reply")
            .expect("socket closed")
            .expect("frame error");
        let Message::Text(text) = frame else { continue };
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("json");
        match parsed["type"].as_str() {
            Some("resumed") | Some("error") => return (replayed, parsed),
            Some("message") => replayed.push(parsed),
            _ => {}
        }
    }
}

/// The happy path, so the rest of the file is measured against a working
/// resume rather than against nothing.
#[tokio::test]
async fn a_live_cursor_replays_what_came_after_it() {
    let (addr, pool) = setup().await;
    insert_message(&pool, MINE, "m-1", "first", "2026-01-01 00:00:01");
    insert_message(&pool, MINE, "m-2", "second", "2026-01-01 00:00:02");
    insert_message(&pool, MINE, "m-3", "third", "2026-01-01 00:00:03");

    let (replayed, ack) = resume(addr, MINE, "m-1").await;

    assert_eq!(ack["type"], "resumed");
    assert_eq!(ack["missedCount"], 2, "two messages followed the cursor");
    assert_eq!(
        ack["cursorLost"], false,
        "the cursor resolved — this is a completed resume"
    );
    let ids: Vec<&str> = replayed
        .iter()
        .map(|m| m["messageId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["m-2", "m-3"]);
}

/// The defect. Retention deletes the message the client is holding as its
/// cursor — an ordinary scheduled event — and the reply used to be
/// indistinguishable from "you are up to date".
#[tokio::test]
async fn a_cursor_deleted_by_retention_is_reported_as_lost_not_as_nothing_missed() {
    let (addr, pool) = setup().await;
    insert_message(&pool, MINE, "m-1", "first", "2026-01-01 00:00:01");
    insert_message(&pool, MINE, "m-2", "second", "2026-01-01 00:00:02");
    insert_message(&pool, MINE, "m-3", "third", "2026-01-01 00:00:03");

    // The client saw m-1 and went offline. Retention then swept it away,
    // exactly as `annex_channels::retention::sweep` does on any channel with
    // a retention window.
    {
        let conn = pool.get().unwrap();
        conn.execute("DELETE FROM messages WHERE message_id = 'm-1'", [])
            .unwrap();
    }

    let (replayed, ack) = resume(addr, MINE, "m-1").await;

    assert_eq!(ack["type"], "resumed");
    assert_eq!(
        ack["cursorLost"], true,
        "the server could not work out what the client missed and must say so — \
         reporting missedCount 0 here tells a client with a hole in its timeline \
         that it is up to date, and nothing ever refetches",
    );
    assert!(
        replayed.is_empty(),
        "a lost cursor cannot replay from an arbitrary point",
    );
}

/// The two identifiers in the frame are independently chosen by the client.
/// The cursor lookup matched on `message_id` alone while membership was
/// checked against `channelId`, so an id belonging to another channel resolved
/// to a perfectly valid timestamp and replayed this channel from that
/// arbitrary point. It is a lost cursor, not a cursor.
#[tokio::test]
async fn a_cursor_from_another_channel_is_lost_not_silently_honoured() {
    let (addr, pool) = setup().await;
    insert_message(&pool, MINE, "m-1", "first", "2026-01-01 00:00:01");
    insert_message(&pool, MINE, "m-2", "second", "2026-01-01 00:00:02");
    insert_message(&pool, MINE, "m-3", "third", "2026-01-01 00:00:03");
    insert_message(&pool, THEIRS, "other-1", "elsewhere", "2026-01-01 00:00:01");

    let (replayed, ack) = resume(addr, MINE, "other-1").await;

    assert_eq!(ack["type"], "resumed");
    assert_eq!(
        ack["cursorLost"], true,
        "a cursor naming a message in a different channel is not a cursor in this one",
    );
    assert!(replayed.is_empty());
}

/// Non-membership is the condition `subscribe` already answers with an error
/// frame. Resume answered it with a successful-looking ack, so someone removed
/// from a channel was told, in that channel, that they were up to date.
#[tokio::test]
async fn resuming_a_channel_you_are_not_in_is_an_error_not_an_ack() {
    let (addr, pool) = setup().await;
    insert_message(&pool, CLOSED, "c-1", "members only", "2026-01-01 00:00:01");

    let (replayed, reply) = resume(addr, CLOSED, "c-1").await;

    assert_eq!(
        reply["type"], "error",
        "expected an error frame, got: {reply}",
    );
    let msg = reply["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("Not a member"),
        "the error must name the reason, got: {msg}",
    );
    assert!(
        replayed.is_empty(),
        "nothing may be replayed to a non-member"
    );
}
