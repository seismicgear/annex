//! `/api/profile/username*` and `/api/usernames/visible` — the privacy boundary.
//!
//! None of these five routes had an integration test, which is the worst place
//! in the app for a coverage gap. Annex's whole claim is that a member is an
//! anonymous cryptographic id unless they explicitly choose otherwise: a
//! username is stored encrypted and revealed only to pseudonyms the owner has
//! granted. Every part of that — that a username is not visible by default,
//! that a grant is directional, that revoking takes it away again — is
//! enforced solely by these handlers.

mod common;

use annex_types::ServerPolicy;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use common::setup_test_app_with_policy;
use std::net::SocketAddr;
use tower::ServiceExt;

fn usernames_enabled() -> ServerPolicy {
    ServerPolicy {
        usernames_enabled: true,
        ..ServerPolicy::default()
    }
}

fn add_member(pool: &annex_db::DbPool, pseudonym: &str) {
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

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    caller: &str,
    body: Option<&str>,
) -> (StatusCode, String) {
    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut req = Request::builder()
        .uri(uri)
        .method(method)
        .header("content-type", "application/json")
        .header("X-Annex-Pseudonym", caller)
        .body(body.map_or_else(Body::empty, |b| Body::from(b.to_string())))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

async fn set_username(app: &axum::Router, who: &str, name: &str) -> StatusCode {
    send(
        app,
        "PUT",
        "/api/profile/username",
        who,
        Some(&format!(r#"{{"username":"{name}"}}"#)),
    )
    .await
    .0
}

async fn grant(app: &axum::Router, granter: &str, grantee: &str) -> StatusCode {
    send(
        app,
        "POST",
        "/api/profile/username/grant",
        granter,
        Some(&format!(r#"{{"grantee_pseudonym":"{grantee}"}}"#)),
    )
    .await
    .0
}

async fn visible_to(app: &axum::Router, who: &str) -> String {
    send(app, "GET", "/api/usernames/visible", who, None)
        .await
        .1
}

// ── The default is invisible ──────────────────────────────────────────────

#[tokio::test]
async fn a_username_is_not_visible_to_anyone_until_it_is_granted() {
    let (app, pool) = setup_test_app_with_policy(usernames_enabled()).await;
    add_member(&pool, "alice");
    add_member(&pool, "bob");

    assert_eq!(set_username(&app, "alice", "Alice").await, StatusCode::OK);

    let bobs_view = visible_to(&app, "bob").await;
    assert!(
        !bobs_view.contains("Alice"),
        "a username must be invisible by default; bob saw {bobs_view}",
    );
}

#[tokio::test]
async fn you_can_always_see_your_own_username() {
    let (app, pool) = setup_test_app_with_policy(usernames_enabled()).await;
    add_member(&pool, "alice");

    set_username(&app, "alice", "Alice").await;

    let view = visible_to(&app, "alice").await;
    assert!(view.contains("Alice"), "own username missing from {view}");
}

// ── Granting ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn a_grant_reveals_the_username_to_exactly_that_pseudonym() {
    let (app, pool) = setup_test_app_with_policy(usernames_enabled()).await;
    for who in ["alice", "bob", "carol"] {
        add_member(&pool, who);
    }

    set_username(&app, "alice", "Alice").await;
    assert_eq!(grant(&app, "alice", "bob").await, StatusCode::OK);

    assert!(
        visible_to(&app, "bob").await.contains("Alice"),
        "the grantee must see it",
    );
    assert!(
        !visible_to(&app, "carol").await.contains("Alice"),
        "a grant to bob must not reveal anything to carol",
    );
}

#[tokio::test]
async fn a_grant_is_one_directional() {
    let (app, pool) = setup_test_app_with_policy(usernames_enabled()).await;
    add_member(&pool, "alice");
    add_member(&pool, "bob");

    set_username(&app, "alice", "Alice").await;
    set_username(&app, "bob", "Bob").await;
    grant(&app, "alice", "bob").await;

    assert!(visible_to(&app, "bob").await.contains("Alice"));
    assert!(
        !visible_to(&app, "alice").await.contains("Bob"),
        "alice granting bob must not silently grant her the reverse",
    );
}

#[tokio::test]
async fn granting_twice_is_not_an_error() {
    let (app, pool) = setup_test_app_with_policy(usernames_enabled()).await;
    add_member(&pool, "alice");
    add_member(&pool, "bob");

    set_username(&app, "alice", "Alice").await;
    assert_eq!(grant(&app, "alice", "bob").await, StatusCode::OK);
    assert_eq!(
        grant(&app, "alice", "bob").await,
        StatusCode::OK,
        "re-granting is the user pressing the button again, not a conflict",
    );
}

// ── Revoking ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn revoking_a_grant_takes_the_username_back() {
    let (app, pool) = setup_test_app_with_policy(usernames_enabled()).await;
    add_member(&pool, "alice");
    add_member(&pool, "bob");

    set_username(&app, "alice", "Alice").await;
    grant(&app, "alice", "bob").await;
    assert!(visible_to(&app, "bob").await.contains("Alice"));

    let (status, _) = send(
        &app,
        "DELETE",
        "/api/profile/username/grant/bob",
        "alice",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    assert!(
        !visible_to(&app, "bob").await.contains("Alice"),
        "revocation has to actually take effect, or the control is theatre",
    );
}

#[tokio::test]
async fn only_the_granter_can_revoke_their_own_grant() {
    let (app, pool) = setup_test_app_with_policy(usernames_enabled()).await;
    for who in ["alice", "bob", "mallory"] {
        add_member(&pool, who);
    }

    set_username(&app, "alice", "Alice").await;
    grant(&app, "alice", "bob").await;

    // Mallory tries to revoke alice's grant to bob. The route is scoped to the
    // caller as granter, so this can only ever touch mallory's own grants.
    let (status, _) = send(
        &app,
        "DELETE",
        "/api/profile/username/grant/bob",
        "mallory",
        None,
    )
    .await;
    assert!(
        status.is_success() || status == StatusCode::NOT_FOUND,
        "unexpected status {status}",
    );
    assert!(
        visible_to(&app, "bob").await.contains("Alice"),
        "a third party must not be able to revoke someone else's grant",
    );
}

// ── Listing your own grants ───────────────────────────────────────────────

#[tokio::test]
async fn listing_grants_shows_who_you_have_revealed_yourself_to() {
    let (app, pool) = setup_test_app_with_policy(usernames_enabled()).await;
    for who in ["alice", "bob", "carol"] {
        add_member(&pool, who);
    }

    set_username(&app, "alice", "Alice").await;
    grant(&app, "alice", "bob").await;

    let (status, body) = send(&app, "GET", "/api/profile/username/grants", "alice", None).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert!(body.contains("bob"), "grant to bob missing from {body}");
    assert!(
        !body.contains("carol"),
        "carol was never granted anything: {body}",
    );
}

#[tokio::test]
async fn your_grant_list_is_yours_alone() {
    let (app, pool) = setup_test_app_with_policy(usernames_enabled()).await;
    for who in ["alice", "bob", "carol"] {
        add_member(&pool, who);
    }

    set_username(&app, "alice", "Alice").await;
    grant(&app, "alice", "bob").await;

    // Carol asks for grants. She should see her own (none), not alice's.
    let (status, body) = send(&app, "GET", "/api/profile/username/grants", "carol", None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("bob"),
        "carol must not learn who alice has revealed herself to: {body}",
    );
}

// ── Policy gate ───────────────────────────────────────────────────────────

#[tokio::test]
async fn usernames_disabled_refuses_to_store_one() {
    // Default policy has usernames off.
    let (app, pool) = setup_test_app_with_policy(ServerPolicy::default()).await;
    add_member(&pool, "alice");

    assert_eq!(
        set_username(&app, "alice", "Alice").await,
        StatusCode::BAD_REQUEST,
        "an operator who turned usernames off must not have them stored anyway",
    );
}

#[tokio::test]
async fn usernames_disabled_returns_an_empty_map_rather_than_an_error() {
    let (app, pool) = setup_test_app_with_policy(ServerPolicy::default()).await;
    add_member(&pool, "alice");

    let (status, body) = send(&app, "GET", "/api/usernames/visible", "alice", None).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the client polls this on every member list render; a hard error there \
         would render the whole roster as broken rather than as anonymous",
    );
    assert!(body.contains("usernames"), "body: {body}");
}
