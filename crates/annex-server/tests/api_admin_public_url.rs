//! `PUT /api/admin/public-url` and `PUT /api/admin/webrtc-public-url`.
//!
//! These two routes had no test, and between them they decide where this
//! server claims to live. The first rewrites the canonical public URL *and*
//! re-derives the server slug from it — the slug that appears in invite
//! links and identifies the instance to federation peers. The second decides
//! the address handed to remote clients joining a call.
//!
//! Both are gated on `can_moderate` alone. A gate that no test exercises is
//! a gate nobody has checked is wired: the branch can be correct and still
//! never be reached, which is how the upload authorization and the voice
//! roster both went wrong. So the first thing asserted here is that an
//! ordinary member cannot reach them, and the second is that the write is
//! visible to the next request — a persisted value nothing reads is the same
//! defect as a check nothing runs.

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

fn add_member(pool: &DbPool, pseudonym: &str, can_moderate: bool) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities
           (server_id, pseudonym_id, participant_type, can_voice, can_moderate,
            can_invite, can_federate, can_bridge, active)
         VALUES (1, ?1, 'HUMAN', 1, ?2, 1, 0, 0, 1)",
        rusqlite::params![pseudonym, can_moderate as i64],
    )
    .unwrap();
}

async fn put(app: &axum::Router, uri: &str, caller: &str, body: Value) -> (StatusCode, String) {
    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut req = Request::builder()
        .uri(uri)
        .method("PUT")
        .header("X-Annex-Pseudonym", caller)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

// ── Authorization ────────────────────────────────────────────────────────

#[tokio::test]
async fn a_plain_member_cannot_move_the_server() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "member", false);

    // The public URL is the origin of every invite link and the identity a
    // federation peer resolves. A member repointing it is a takeover of the
    // instance's name, not a preference.
    let (status, body) = put(
        &app,
        "/api/admin/public-url",
        "member",
        serde_json::json!({ "public_url": "https://evil.example.com" }),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
}

#[tokio::test]
async fn a_plain_member_cannot_repoint_voice() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "member", false);

    let (status, body) = put(
        &app,
        "/api/admin/webrtc-public-url",
        "member",
        serde_json::json!({ "public_webrtc_url": "wss://evil.example.com" }),
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN, "body: {body}");
}

#[tokio::test]
async fn an_unauthenticated_caller_cannot_reach_either_route() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "boss", true);

    for uri in ["/api/admin/public-url", "/api/admin/webrtc-public-url"] {
        let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        let mut req = Request::builder()
            .uri(uri)
            .method("PUT")
            .header("content-type", "application/json")
            .body(Body::from("{\"public_url\":\"https://x.example.com\"}"))
            .unwrap();
        req.extensions_mut().insert(ConnectInfo(addr));

        let status = app.clone().oneshot(req).await.unwrap().status();
        assert_ne!(status, StatusCode::OK, "{uri} served an anonymous caller");
    }
}

// ── The write, and whether anything can see it ───────────────────────────

#[tokio::test]
async fn a_moderator_can_set_the_public_url_and_the_slug_follows() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "boss", true);

    let (status, body) = put(
        &app,
        "/api/admin/public-url",
        "boss",
        serde_json::json!({ "public_url": "https://annex.example.com" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let json: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        json["public_url"].as_str(),
        Some("https://annex.example.com")
    );

    // The slug is derived from the URL, and it is what appears in invite
    // links and identifies this instance to peers. A response that reports a
    // slug the database does not hold would be believed by the admin panel
    // and by nothing else.
    let slug = json["server_slug"]
        .as_str()
        .expect("server_slug in response");
    let conn = pool.get().unwrap();
    let (stored_url, stored_slug): (String, String) = conn
        .query_row(
            "SELECT public_url, slug FROM servers WHERE id = 1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored_url, "https://annex.example.com");
    assert_eq!(stored_slug, slug, "the reported slug is not the stored one");
}

#[tokio::test]
async fn a_trailing_slash_does_not_produce_a_different_origin() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "boss", true);

    // Invite links and federation origins are compared as strings in places,
    // so "https://x.example.com" and "https://x.example.com/" must not be
    // two different servers.
    let (status, body) = put(
        &app,
        "/api/admin/public-url",
        "boss",
        serde_json::json!({ "public_url": "  https://annex.example.com/  " }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let json: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        json["public_url"].as_str(),
        Some("https://annex.example.com")
    );
}

#[tokio::test]
async fn a_url_with_no_scheme_is_refused() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "boss", true);

    // A bare host would be persisted and then concatenated into invite links
    // that resolve nowhere.
    let (status, body) = put(
        &app,
        "/api/admin/public-url",
        "boss",
        serde_json::json!({ "public_url": "annex.example.com" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn a_moderator_can_set_the_webrtc_public_url() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "boss", true);

    let (status, body) = put(
        &app,
        "/api/admin/webrtc-public-url",
        "boss",
        serde_json::json!({ "public_webrtc_url": "wss://voice.example.com" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    let json: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        json["public_webrtc_url"].as_str(),
        Some("wss://voice.example.com"),
    );
}

#[tokio::test]
async fn a_webrtc_url_with_an_unusable_scheme_is_refused() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "boss", true);

    let (status, body) = put(
        &app,
        "/api/admin/webrtc-public-url",
        "boss",
        serde_json::json!({ "public_webrtc_url": "ftp://voice.example.com" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}
