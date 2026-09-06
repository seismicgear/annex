//! `GET /api/channels/{id}/voice/status` — the shape of the response.
//!
//! This exists because of a bug that nothing else could have caught. The
//! handler hand-built its JSON with a `json!` literal that re-listed the
//! fields, so when `participant_ids` was added to `VoiceStatusResponse` it
//! serialized correctly in the service and was then silently dropped at the
//! edge. Everything still compiled, every unit test still passed, and the
//! client — which defaults a missing roster to `[]` — could not tell the
//! difference between "the server sent no roster" and "nobody is in the call".
//! The whole feature was inert and looked fine.
//!
//! Unit tests on the service could not see it, because the service was right.
//! Component tests could not see it, because they inject the store directly.
//! The browser audit could not see it, because a solo call has an empty roster
//! either way. It was only visible at the HTTP boundary, so the test lives
//! here, and it asserts the field is *present* rather than merely well-formed.

mod common;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use common::setup_test_app;
use serde_json::Value;
use std::net::SocketAddr;
use tower::ServiceExt;

fn add_member(pool: &annex_db::DbPool, pseudonym: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities
           (server_id, pseudonym_id, participant_type, can_voice, can_moderate,
            can_invite, can_federate, can_bridge, active)
         VALUES (1, ?1, 'HUMAN', 1, 1, 1, 0, 0, 1)",
        [pseudonym],
    )
    .unwrap();
}

/// A channel the member belongs to, so `require_membership` passes.
fn add_channel(pool: &annex_db::DbPool, channel_id: &str, member: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO channels (channel_id, server_id, name, channel_type, federation_scope)
         VALUES (?1, 1, 'audit-voice', 'Voice', 'LOCAL_ONLY')",
        [channel_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO channel_members (channel_id, pseudonym_id, server_id)
         VALUES (?1, ?2, 1)",
        [channel_id, member],
    )
    .unwrap();
}

async fn voice_status(app: &axum::Router, caller: &str, channel: &str) -> (StatusCode, String) {
    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut req = Request::builder()
        .uri(format!("/api/channels/{channel}/voice/status"))
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
async fn the_response_carries_the_participant_roster() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan-voice", "alice");

    let (status, body) = voice_status(&app, "alice", "chan-voice").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let json: Value = serde_json::from_str(&body).unwrap_or_else(|e| panic!("{e}: {body}"));

    // The field has to be PRESENT. A client defaulting a missing roster to an
    // empty list cannot distinguish absence from emptiness, which is exactly
    // how this went unnoticed.
    assert!(
        json.get("participant_ids").is_some(),
        "participant_ids missing from the response — the client silently \
         defaults it to [] and renders an empty call: {body}",
    );
    assert!(
        json["participant_ids"].is_array(),
        "participant_ids must be an array: {body}",
    );
}

#[tokio::test]
async fn the_response_carries_the_count_and_active_flag() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan-voice", "alice");

    let (_, body) = voice_status(&app, "alice", "chan-voice").await;
    let json: Value = serde_json::from_str(&body).unwrap();

    assert!(json.get("participants").is_some(), "body: {body}");
    assert!(json.get("active").is_some(), "body: {body}");
}

/// Guards the whole shape at once, so a field added to `VoiceStatusResponse`
/// and dropped at the edge fails here rather than shipping inert.
#[tokio::test]
async fn the_response_shape_matches_the_struct() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan-voice", "alice");

    let (_, body) = voice_status(&app, "alice", "chan-voice").await;
    let json: Value = serde_json::from_str(&body).unwrap();
    let mut keys: Vec<&str> = json
        .as_object()
        .unwrap()
        .keys()
        .map(|k| k.as_str())
        .collect();
    keys.sort_unstable();

    assert_eq!(
        keys,
        ["active", "participant_ids", "participants"],
        "the serialized response drifted from VoiceStatusResponse: {body}",
    );
}

#[tokio::test]
async fn a_non_member_cannot_read_voice_status() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_member(&pool, "stranger");
    add_channel(&pool, "chan-voice", "alice");

    let (status, _) = voice_status(&app, "stranger", "chan-voice").await;
    assert_ne!(
        status,
        StatusCode::OK,
        "voice status names who is in a call; it is membership-gated",
    );
}
