//! `GET /api/messages/search` — the route with the widest blast radius and
//! no test at all.
//!
//! Search is the one read path that is not scoped by a channel in the URL.
//! Every other message route names the channel, so `require_membership` has
//! something to check against; search has to *derive* its scope, and if it
//! derives it wrong the failure is silent and total: a member of one channel
//! types a common word and reads private conversations they were never in.
//! Nothing about that looks broken from the outside. The response is a
//! well-formed 200 with plausible messages in it.
//!
//! So the tests here are mostly about what must NOT come back, plus the
//! bounds (empty query, over-long query, page cap) that decide whether a
//! search box can be used to make the server scan the whole database.

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

/// Creates a channel. Membership is granted separately so a test can create
/// a channel the caller is deliberately *not* in.
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

/// Seeds a message with an explicit timestamp, because ordering across
/// channels is part of the contract and `datetime('now')` has one-second
/// resolution — several messages seeded in a test would otherwise tie.
fn say(pool: &DbPool, channel_id: &str, sender: &str, id: &str, content: &str, at: &str) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO messages (server_id, channel_id, message_id, sender_pseudonym, content, created_at)
         VALUES (1, ?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![channel_id, id, sender, content, at],
    )
    .unwrap();
}

async fn search(app: &axum::Router, caller: &str, query: &str) -> (StatusCode, String) {
    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut req = Request::builder()
        .uri(format!("/api/messages/search?{query}"))
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

fn contents(body: &str) -> Vec<String> {
    let json: Value = serde_json::from_str(body).unwrap_or_else(|e| panic!("{e}: {body}"));
    json.as_array()
        .unwrap_or_else(|| panic!("expected an array: {body}"))
        .iter()
        .map(|m| m["content"].as_str().unwrap_or_default().to_string())
        .collect()
}

// ── Scope: the thing that must never leak ────────────────────────────────

#[tokio::test]
async fn an_unscoped_search_only_reaches_channels_the_caller_is_in() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_member(&pool, "bob");

    add_channel(&pool, "public-chan");
    add_channel(&pool, "private-chan");
    join(&pool, "public-chan", "alice");
    join(&pool, "private-chan", "bob");

    say(
        &pool,
        "public-chan",
        "alice",
        "m1",
        "the widget ships tuesday",
        "2026-01-01 10:00:00",
    );
    say(
        &pool,
        "private-chan",
        "bob",
        "m2",
        "the widget budget is confidential",
        "2026-01-01 11:00:00",
    );

    let (status, body) = search(&app, "alice", "q=widget").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let found = contents(&body);
    assert!(
        found.iter().any(|c| c.contains("ships tuesday")),
        "alice cannot find her own channel's message: {body}",
    );
    assert!(
        !found.iter().any(|c| c.contains("confidential")),
        "search returned a message from a channel the caller is not in — \
         this is a full read of every private channel on the server: {body}",
    );
}

#[tokio::test]
async fn a_search_targeted_at_a_channel_the_caller_is_not_in_is_refused() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_member(&pool, "bob");
    add_channel(&pool, "private-chan");
    join(&pool, "private-chan", "bob");
    say(
        &pool,
        "private-chan",
        "bob",
        "m1",
        "secret plans",
        "2026-01-01 10:00:00",
    );

    let (status, body) = search(&app, "alice", "q=secret&channel_id=private-chan").await;
    assert_ne!(
        status,
        StatusCode::OK,
        "naming the channel explicitly bypassed the membership check: {body}",
    );
}

#[tokio::test]
async fn a_caller_in_no_channels_gets_an_empty_result_rather_than_everything() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_member(&pool, "bob");
    add_channel(&pool, "somewhere");
    join(&pool, "somewhere", "bob");
    say(
        &pool,
        "somewhere",
        "bob",
        "m1",
        "hello world",
        "2026-01-01 10:00:00",
    );

    let (status, body) = search(&app, "alice", "q=hello").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        contents(&body).is_empty(),
        "a user with no memberships saw content: {body}",
    );
}

// ── Matching ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn matching_ignores_case() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan");
    join(&pool, "chan", "alice");
    say(
        &pool,
        "chan",
        "alice",
        "m1",
        "Deployment Window is open",
        "2026-01-01 10:00:00",
    );

    let (status, body) = search(&app, "alice", "q=deployment%20window").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(
        contents(&body).len(),
        1,
        "a case-sensitive search box finds nothing for most of what people \
         actually type: {body}",
    );
}

#[tokio::test]
async fn a_search_that_matches_nothing_is_an_empty_list_not_an_error() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan");
    join(&pool, "chan", "alice");
    say(
        &pool,
        "chan",
        "alice",
        "m1",
        "good morning",
        "2026-01-01 10:00:00",
    );

    let (status, body) = search(&app, "alice", "q=zzzznotpresent").await;

    // "No results" and "the search broke" render identically if the server
    // conflates them, and the client cannot tell the user which happened.
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(contents(&body).is_empty(), "body: {body}");
}

#[tokio::test]
async fn results_span_every_channel_the_caller_belongs_to_newest_first() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan-a");
    add_channel(&pool, "chan-b");
    join(&pool, "chan-a", "alice");
    join(&pool, "chan-b", "alice");

    say(
        &pool,
        "chan-a",
        "alice",
        "m1",
        "release note one",
        "2026-01-01 10:00:00",
    );
    say(
        &pool,
        "chan-b",
        "alice",
        "m2",
        "release note two",
        "2026-01-02 10:00:00",
    );

    let (status, body) = search(&app, "alice", "q=release%20note").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let found = contents(&body);
    assert_eq!(found.len(), 2, "the sweep missed a channel: {body}");
    assert!(
        found[0].contains("two"),
        "results are not newest-first across channels, so paging with a \
         limit would silently drop the most recent hits: {body}",
    );
}

// ── Bounds ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_empty_query_is_rejected() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan");
    join(&pool, "chan", "alice");

    // An empty needle matches every message in every channel the caller is
    // in — a full export dressed up as a search.
    let (status, body) = search(&app, "alice", "q=").await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn a_whitespace_only_query_is_rejected() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan");
    join(&pool, "chan", "alice");

    let (status, body) = search(&app, "alice", "q=%20%20%20").await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "spaces are not a search term, and they match every message: {body}",
    );
}

#[tokio::test]
async fn an_over_long_query_is_rejected() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan");
    join(&pool, "chan", "alice");

    let long = "a".repeat(201);
    let (status, body) = search(&app, "alice", &format!("q={long}")).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body: {body}");
}

#[tokio::test]
async fn a_query_at_the_length_limit_is_accepted() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan");
    join(&pool, "chan", "alice");

    // The boundary itself, so the limit cannot drift by one without being
    // noticed in one direction or the other.
    let at_limit = "a".repeat(200);
    let (status, body) = search(&app, "alice", &format!("q={at_limit}")).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
}

#[tokio::test]
async fn a_missing_query_parameter_is_a_client_error() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    let (status, body) = search(&app, "alice", "channel_id=chan").await;
    assert!(
        status.is_client_error(),
        "a request with no `q` should be rejected, not served: {status} {body}",
    );
}

#[tokio::test]
async fn the_caller_cannot_ask_for_an_unbounded_page() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan");
    join(&pool, "chan", "alice");

    for i in 0..60 {
        say(
            &pool,
            "chan",
            "alice",
            &format!("m{i}"),
            "needle in the haystack",
            &format!("2026-01-01 10:{:02}:00", i % 60),
        );
    }

    let (status, body) = search(&app, "alice", "q=needle&limit=5000").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(
        contents(&body).len() <= 50,
        "the page cap is not applied, so a single request can pull the whole \
         history into memory: {} results",
        contents(&body).len(),
    );
}

#[tokio::test]
async fn an_explicit_limit_below_the_cap_is_honoured() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan");
    join(&pool, "chan", "alice");

    for i in 0..10 {
        say(
            &pool,
            "chan",
            "alice",
            &format!("m{i}"),
            "needle",
            &format!("2026-01-01 10:{i:02}:00"),
        );
    }

    let (status, body) = search(&app, "alice", "q=needle&limit=3").await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(contents(&body).len(), 3, "body: {body}");
}

// ── Authentication ───────────────────────────────────────────────────────

#[tokio::test]
async fn an_unauthenticated_search_is_refused() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");
    add_channel(&pool, "chan");
    join(&pool, "chan", "alice");
    say(
        &pool,
        "chan",
        "alice",
        "m1",
        "anything at all",
        "2026-01-01 10:00:00",
    );

    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut req = Request::builder()
        .uri("/api/messages/search?q=anything")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.oneshot(req).await.unwrap();
    assert_ne!(
        resp.status(),
        StatusCode::OK,
        "search served a caller with no identity",
    );
}
