//! `GET /api/channels/{channelId}/messages/{messageId}/edits`.
//!
//! The route takes two identifiers and only one of them was ever checked.
//! `require_membership` was called against the channel in the *path*, and
//! then the edit history was fetched with `WHERE message_id = ?` and no
//! channel constraint at all — so the path channel decided whether you were
//! allowed in, and the message id decided what you got. Point the first at
//! a channel you belong to and the second at a message from a channel you
//! do not, and the server hands over the edit history: every prior version
//! of a message in a private channel, decrypted.
//!
//! Nothing above the query could catch this. The membership check is real,
//! it runs, and it passes — it is just answering a different question from
//! the one the response depends on.

mod common;

use annex_db::DbPool;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use common::setup_test_app;
use serde_json::Value;
use std::net::SocketAddr;
use tower::ServiceExt;

fn add_member(pool: &DbPool, pseudonym: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities
           (server_id, pseudonym_id, participant_type, can_voice, can_moderate,
            can_invite, can_federate, can_bridge, active)
         VALUES (1, ?1, 'HUMAN', 1, 0, 1, 0, 0, 1)",
        [pseudonym],
    )
    .unwrap();
}

fn add_channel(pool: &DbPool, channel_id: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO channels (channel_id, server_id, name, channel_type, federation_scope)
         VALUES (?1, 1, ?1, 'Text', 'LOCAL_ONLY')",
        [channel_id],
    )
    .unwrap();
}

fn join(pool: &DbPool, channel_id: &str, pseudonym: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO channel_members (channel_id, pseudonym_id, server_id)
         VALUES (?1, ?2, 1)",
        [channel_id, pseudonym],
    )
    .unwrap();
}

/// Seeds a message plus one prior version of it in `message_edits`.
fn say_and_edit(pool: &DbPool, channel_id: &str, sender: &str, id: &str, was: &str, now: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO messages (server_id, channel_id, message_id, sender_pseudonym, content)
         VALUES (1, ?1, ?2, ?3, ?4)",
        rusqlite::params![channel_id, id, sender, now],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO message_edits (message_id, old_content, edited_at)
         VALUES (?1, ?2, datetime('now'))",
        rusqlite::params![id, was],
    )
    .unwrap();
}

async fn edits(
    app: &axum::Router,
    caller: &str,
    channel: &str,
    message: &str,
) -> (StatusCode, String) {
    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut req = Request::builder()
        .uri(format!("/api/channels/{channel}/messages/{message}/edits"))
        .method("GET")
        .header("X-Annex-Pseudonym", caller)
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

#[tokio::test]
async fn a_member_can_read_the_edit_history_of_a_message_in_their_own_channel() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan-a");
    join(&pool, "chan-a", "alice");
    say_and_edit(&pool, "chan-a", "alice", "m1", "first draft", "final text");

    let (status, body) = edits(&app, "alice", "chan-a", "m1").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let json: Value = serde_json::from_str(&body).unwrap();
    let arr = json.as_array().expect("array");
    assert_eq!(arr.len(), 1, "body: {body}");
    assert_eq!(arr[0]["old_content"].as_str(), Some("first draft"));
}

#[tokio::test]
async fn the_edit_history_of_another_channels_message_is_not_readable() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_member(&pool, "bob");

    add_channel(&pool, "chan-alice");
    add_channel(&pool, "chan-private");
    join(&pool, "chan-alice", "alice");
    join(&pool, "chan-private", "bob");

    // Bob's message, in a channel alice is not in, with a prior version.
    say_and_edit(
        &pool,
        "chan-private",
        "bob",
        "m-secret",
        "the salary figure is 250k",
        "redacted",
    );

    // Alice names a channel she IS in, and a message she is not entitled to.
    // The membership check passes; the query must not.
    let (status, body) = edits(&app, "alice", "chan-alice", "m-secret").await;
    assert!(
        !body.contains("salary"),
        "the prior version of a private message leaked: {body}",
    );

    // Deliberately an empty 200 rather than a 403. Refusing a message id
    // that exists elsewhere, while returning an empty list for one that
    // exists nowhere, tells the caller which ids are real — the two cases
    // have to be indistinguishable, and "there is no such message in this
    // channel" is true of both. See
    // `an_unknown_message_id_does_not_serve_an_empty_success`, which pins
    // the other half of that pair.
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let json: Value = serde_json::from_str(&body).unwrap();
    assert!(
        json.as_array().unwrap().is_empty(),
        "the response distinguishes a foreign message from an unknown one, \
         which is an oracle for which message ids exist: {body}",
    );
}

#[tokio::test]
async fn naming_a_channel_you_are_not_in_is_still_refused() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_member(&pool, "bob");
    add_channel(&pool, "chan-private");
    join(&pool, "chan-private", "bob");
    say_and_edit(&pool, "chan-private", "bob", "m1", "before", "after");

    // The straightforward attempt, which the membership check already
    // handled. Kept so a fix to the query cannot be mistaken for the whole
    // guard: both halves have to hold.
    let (status, body) = edits(&app, "alice", "chan-private", "m1").await;
    assert_ne!(status, StatusCode::OK, "body: {body}");
}

#[tokio::test]
async fn a_message_that_was_never_edited_has_an_empty_history() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan-a");
    join(&pool, "chan-a", "alice");
    {
        let conn = pool.get().unwrap();
        conn.execute(
            "INSERT INTO messages (server_id, channel_id, message_id, sender_pseudonym, content)
             VALUES (1, 'chan-a', 'm-plain', 'alice', 'never touched')",
            [],
        )
        .unwrap();
    }

    // "No edits" and "no such message" must not be the same answer to the
    // caller, but neither may be an error: an unedited message is the
    // normal case and the client renders it as "no history".
    let (status, body) = edits(&app, "alice", "chan-a", "m-plain").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let json: Value = serde_json::from_str(&body).unwrap();
    assert!(json.as_array().unwrap().is_empty(), "body: {body}");
}

#[tokio::test]
async fn an_unknown_message_id_does_not_serve_an_empty_success() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan-a");
    join(&pool, "chan-a", "alice");

    // A message id that exists nowhere. This is the probe an attacker uses
    // to enumerate: if an unknown id and a foreign id give different
    // answers, the difference is an oracle for which ids are real.
    let (status, body) = edits(&app, "alice", "chan-a", "m-does-not-exist").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "an unknown id should look exactly like an unedited one: {body}",
    );
    let json: Value = serde_json::from_str(&body).unwrap();
    assert!(json.as_array().unwrap().is_empty(), "body: {body}");
}
