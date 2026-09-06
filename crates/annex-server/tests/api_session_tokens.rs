//! `POST /api/ws/token` and `POST /api/session/refresh` — the token lifecycle.
//!
//! Neither route had an integration test. Both mint credentials, and
//! `refresh_session_handler` is the one place in the app that deliberately
//! bypasses `auth_middleware`: it has to accept an *expired* token, because
//! accepting one is the entire point of a refresh endpoint. That makes its
//! remaining checks — a valid HMAC signature, and an identity that is still
//! active — the only thing standing between a stale token and an indefinite
//! session. A regression there would not fail loudly; it would quietly keep
//! deactivated accounts signed in.

mod common;

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use common::setup_test_app;
use std::net::SocketAddr;
use tower::ServiceExt;

fn add_member(pool: &annex_db::DbPool, pseudonym: &str, active: bool) {
    let conn = pool.get().unwrap();
    conn.execute(
        "INSERT INTO platform_identities
           (server_id, pseudonym_id, participant_type, can_voice, can_moderate,
            can_invite, can_federate, can_bridge, active)
         VALUES (1, ?1, 'HUMAN', 1, 0, 1, 0, 0, ?2)",
        rusqlite::params![pseudonym, active as i64],
    )
    .unwrap();
}

async fn post(app: &axum::Router, uri: &str, headers: &[(&str, &str)]) -> (StatusCode, String) {
    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut builder = Request::builder().uri(uri).method("POST");
    for (k, v) in headers {
        builder = builder.header(*k, *v);
    }
    let mut req = builder.body(Body::empty()).unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

fn field(body: &str, key: &str) -> String {
    serde_json::from_str::<serde_json::Value>(body)
        .unwrap_or_else(|e| panic!("body was not JSON ({e}): {body}"))
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("no `{key}` in {body}"))
        .to_string()
}

// ── Minting a WebSocket token ─────────────────────────────────────────────

#[tokio::test]
async fn an_authenticated_member_gets_a_ws_token() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice", true);

    let (status, body) = post(&app, "/api/ws/token", &[("X-Annex-Pseudonym", "alice")]).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");

    let token = field(&body, "token");
    assert!(!token.is_empty());
    assert!(
        body.contains("expires_in_secs"),
        "the client needs the TTL to schedule a refresh: {body}",
    );
}

#[tokio::test]
async fn an_unauthenticated_caller_gets_no_ws_token() {
    let (app, _pool) = setup_test_app().await;

    let (status, _) = post(&app, "/api/ws/token", &[]).await;
    assert!(
        status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
        "a WS token is a credential; unauthenticated callers must not mint one (got {status})",
    );
}

#[tokio::test]
async fn a_token_is_bound_to_the_pseudonym_that_asked_for_it() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice", true);
    add_member(&pool, "bob", true);

    let (_, alice) = post(&app, "/api/ws/token", &[("X-Annex-Pseudonym", "alice")]).await;
    let (_, bob) = post(&app, "/api/ws/token", &[("X-Annex-Pseudonym", "bob")]).await;

    assert_ne!(
        field(&alice, "token"),
        field(&bob, "token"),
        "two members must not receive interchangeable tokens",
    );
}

// ── Refreshing a session ──────────────────────────────────────────────────

#[tokio::test]
async fn refresh_exchanges_a_valid_token_for_a_fresh_one() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice", true);

    let (_, minted) = post(&app, "/api/ws/token", &[("X-Annex-Pseudonym", "alice")]).await;
    let token = field(&minted, "token");

    let (status, body) = post(
        &app,
        "/api/session/refresh",
        &[("Authorization", &format!("Bearer {token}"))],
    )
    .await;

    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(!field(&body, "sessionToken").is_empty());
}

#[tokio::test]
async fn refresh_without_an_authorization_header_is_rejected() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice", true);

    let (status, _) = post(&app, "/api/session/refresh", &[]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn refresh_requires_the_bearer_scheme() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice", true);

    let (_, minted) = post(&app, "/api/ws/token", &[("X-Annex-Pseudonym", "alice")]).await;
    let token = field(&minted, "token");

    // Same token, no scheme.
    let (status, _) = post(&app, "/api/session/refresh", &[("Authorization", &token)]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn refresh_rejects_a_token_this_server_did_not_sign() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice", true);

    // Shaped like a token, signed by nobody. This is the check that stops
    // anyone from writing their own pseudonym into a session.
    let (status, _) = post(
        &app,
        "/api/session/refresh",
        &[("Authorization", "Bearer alice.9999999999.deadbeefdeadbeef")],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an unsigned token must never refresh into a valid session",
    );
}

#[tokio::test]
async fn refresh_rejects_a_tampered_signature() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice", true);

    let (_, minted) = post(&app, "/api/ws/token", &[("X-Annex-Pseudonym", "alice")]).await;
    let token = field(&minted, "token");

    // Flip the last character of the signature.
    let mut tampered = token.clone();
    let last = tampered.pop().unwrap();
    tampered.push(if last == 'a' { 'b' } else { 'a' });

    let (status, _) = post(
        &app,
        "/api/session/refresh",
        &[("Authorization", &format!("Bearer {tampered}"))],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

/// The check that matters most, because it is the only one an expired-token
/// path still performs against live state.
#[tokio::test]
async fn a_deactivated_member_cannot_refresh_their_way_back_in() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice", true);

    let (_, minted) = post(&app, "/api/ws/token", &[("X-Annex-Pseudonym", "alice")]).await;
    let token = field(&minted, "token");

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "UPDATE platform_identities SET active = 0 WHERE pseudonym_id = 'alice'",
            [],
        )
        .unwrap();
    }

    let (status, _) = post(
        &app,
        "/api/session/refresh",
        &[("Authorization", &format!("Bearer {token}"))],
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "deactivating an account has to end its sessions, not just stop new ones",
    );
}

#[tokio::test]
async fn refresh_rejects_a_token_for_an_identity_that_does_not_exist() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice", true);

    let (_, minted) = post(&app, "/api/ws/token", &[("X-Annex-Pseudonym", "alice")]).await;
    let token = field(&minted, "token");

    {
        let conn = pool.get().unwrap();
        conn.execute(
            "DELETE FROM platform_identities WHERE pseudonym_id = 'alice'",
            [],
        )
        .unwrap();
    }

    let (status, _) = post(
        &app,
        "/api/session/refresh",
        &[("Authorization", &format!("Bearer {token}"))],
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
