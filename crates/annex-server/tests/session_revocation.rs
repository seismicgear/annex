//! Revoking a session must actually stop the token working.
//!
//! The unit tests in `ws::tokens` prove the epoch survives a round trip and
//! cannot be edited by its holder. Neither of those is the property that
//! matters: what matters is that a token which authenticated a second ago
//! stops authenticating after a revoke, on every surface that accepts one.
//!
//! Before the epoch existed there was no way to get there. A token is an HMAC
//! over `pseudonym|expires`, verified with no database read, so the options
//! were deactivating the identity — which also stops them re-authenticating —
//! or rotating the server signing key, which invalidates every session on the
//! server. And a leaked token was effectively permanent: `/api/session/refresh`
//! is public by design (its job is to accept an expired token), so anyone
//! holding one could refresh it indefinitely.

mod common;

use axum::body::Body;
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use std::net::SocketAddr;
use tower::ServiceExt;

/// Mint a token for `pseudonym` at `epoch` using the app's own secret.
fn token_for(secret: &[u8; 32], pseudonym: &str, epoch: i64) -> String {
    annex_server::api_ws::generate_session_token(
        pseudonym,
        secret,
        annex_server::api_ws::SESSION_TOKEN_TTL_SECS,
        epoch,
    )
}

/// Every request carries a `ConnectInfo`, because without one
/// `rate_limit_middleware` has no key for an unauthenticated route and answers
/// **500**. That matters here rather than being boilerplate: the first draft of
/// this file asserted `assert_ne!(status, UNAUTHORIZED)` on a route that was
/// returning 500 for all four tests, and three of them passed. An assertion
/// that a request did not fail in one specific way is not an assertion that it
/// succeeded — the same defect this repo's harness notes describe in
/// `postFreshMessage`. Every check below names the exact status it expects.
fn signed_request(method: &str, uri: &str, token: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("Authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(
        "127.0.0.1:40000".parse::<SocketAddr>().unwrap(),
    ));
    req
}

async fn status_of(app: &axum::Router, req: Request<Body>) -> StatusCode {
    app.clone()
        .oneshot(req)
        .await
        .expect("request should complete")
        .status()
}

/// `GET /api/channels` with a session token. Authenticated but not
/// proof-gated, so a valid token yields 200 and the only other status this
/// surface should ever produce is 401.
async fn list_channels(app: &axum::Router, token: &str) -> StatusCode {
    status_of(app, signed_request("GET", "/api/channels", token)).await
}

#[tokio::test]
async fn revoking_sessions_stops_a_token_that_worked_a_moment_ago() {
    let f = common::revocation::fixture().await;

    // Baseline: the token authenticates.
    assert_eq!(
        list_channels(&f.app, &f.token).await,
        StatusCode::OK,
        "the fixture's token should authenticate before revocation"
    );

    // Revoke.
    let epoch = common::revocation::revoke(&f.pool, f.server_id, &f.pseudonym);
    assert_eq!(epoch, 1, "the first revoke should move the epoch to 1");

    // The same token, unchanged, is now refused.
    assert_eq!(
        list_channels(&f.app, &f.token).await,
        StatusCode::UNAUTHORIZED,
        "a token from a revoked epoch must not authenticate"
    );

    // And a token minted against the new epoch works again — revocation
    // invalidates credentials, it does not lock the identity out.
    let fresh = token_for(&f.ws_token_secret, &f.pseudonym, epoch);
    assert_eq!(
        list_channels(&f.app, &fresh).await,
        StatusCode::OK,
        "re-authenticating after a revoke must work"
    );
}

/// The hole this closes. `/api/session/refresh` is public and accepts an
/// expired token on purpose, so without an epoch check there it would hand a
/// revoked token a fresh one and undo the revocation.
#[tokio::test]
async fn a_revoked_token_cannot_refresh_its_way_back_in() {
    let f = common::revocation::fixture().await;

    assert_eq!(
        status_of(
            &f.app,
            signed_request("POST", "/api/session/refresh", &f.token)
        )
        .await,
        StatusCode::OK,
        "refresh should work before revocation"
    );

    common::revocation::revoke(&f.pool, f.server_id, &f.pseudonym);

    assert_eq!(
        status_of(
            &f.app,
            signed_request("POST", "/api/session/refresh", &f.token)
        )
        .await,
        StatusCode::UNAUTHORIZED,
        "a revoked token must not be refreshable — otherwise revocation is undone \
         by the next refresh the client makes on its own timer"
    );
}

/// A refresh mints against the CURRENT epoch, not the one the presented token
/// carried. Minting against the token's own epoch would be a no-op rename that
/// silently kept a stale credential alive across the next revoke.
#[tokio::test]
async fn a_refreshed_token_carries_the_current_epoch() {
    let f = common::revocation::fixture().await;

    // Revoke first, then re-authenticate at the new epoch and refresh.
    let epoch = common::revocation::revoke(&f.pool, f.server_id, &f.pseudonym);
    let live = token_for(&f.ws_token_secret, &f.pseudonym, epoch);

    let resp = f
        .app
        .clone()
        .oneshot(signed_request("POST", "/api/session/refresh", &live))
        .await
        .expect("request should complete");
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024)
        .await
        .expect("body should read");
    let json: serde_json::Value = serde_json::from_slice(&bytes).expect("body should be JSON");
    let minted = json["sessionToken"]
        .as_str()
        .expect("sessionToken")
        .to_string();

    // The minted token must be usable, and must stop working on the next
    // revoke — which is only true if it carries epoch 1 rather than epoch 0.
    assert_eq!(
        list_channels(&f.app, &minted).await,
        StatusCode::OK,
        "a freshly refreshed token must authenticate"
    );
    common::revocation::revoke(&f.pool, f.server_id, &f.pseudonym);
    assert_eq!(
        list_channels(&f.app, &minted).await,
        StatusCode::UNAUTHORIZED,
        "the refreshed token must be caught by the next revoke"
    );
}

/// Migration 044 must not sign the whole server out on upgrade.
#[tokio::test]
async fn a_pre_migration_token_still_authenticates() {
    let f = common::revocation::fixture().await;

    // Epoch 0 is both what a v1 token reads as and the column's default, so
    // every existing session keeps working across the upgrade.
    let legacy = token_for(&f.ws_token_secret, &f.pseudonym, 0);
    assert_eq!(
        list_channels(&f.app, &legacy).await,
        StatusCode::OK,
        "an epoch-0 token must still authenticate against a never-revoked identity"
    );
}

/// Revoking one identity must not touch anybody else's sessions. That is the
/// entire reason this is a per-identity counter rather than a key rotation.
#[tokio::test]
async fn revoking_one_identity_leaves_the_others_alone() {
    let f = common::revocation::fixture().await;
    let other = common::revocation::add_member(&f.pool, f.server_id, "bbbbbbbbbbbb");
    let other_token = token_for(&f.ws_token_secret, &other, 0);

    common::revocation::revoke(&f.pool, f.server_id, &f.pseudonym);

    assert_eq!(
        list_channels(&f.app, &other_token).await,
        StatusCode::OK,
        "revoking one member must not invalidate another member's token"
    );
}

/// Deactivating an identity bumps the epoch too, so a token stops working the
/// moment the account is disabled rather than at its natural expiry.
#[tokio::test]
async fn deactivation_revokes_as_well_as_locks_out() {
    let f = common::revocation::fixture().await;
    assert_eq!(list_channels(&f.app, &f.token).await, StatusCode::OK);

    {
        let conn = f.pool.get().unwrap();
        annex_identity::platform::deactivate_platform_identity(&conn, f.server_id, &f.pseudonym)
            .expect("deactivate should succeed");
    }

    assert_eq!(
        list_channels(&f.app, &f.token).await,
        StatusCode::UNAUTHORIZED,
        "a deactivated identity's token must be refused"
    );

    // And re-activating does not resurrect the old token: the epoch moved.
    {
        let conn = f.pool.get().unwrap();
        conn.execute(
            "UPDATE platform_identities SET active = 1 WHERE server_id = ?1 AND pseudonym_id = ?2",
            rusqlite::params![f.server_id, f.pseudonym],
        )
        .unwrap();
    }
    assert_eq!(
        list_channels(&f.app, &f.token).await,
        StatusCode::UNAUTHORIZED,
        "re-activating an identity must not un-revoke the tokens its deactivation killed"
    );
}
