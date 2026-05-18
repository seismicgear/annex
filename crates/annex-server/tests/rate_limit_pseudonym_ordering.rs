//! Integration tests for middleware ordering: authenticated/protected
//! requests must be rate-limited PER PSEUDONYM, not per IP. Unauthenticated
//! requests must still be rate-limited per IP.
//!
//! These tests exercise the real router stack via `tower::ServiceExt::oneshot`,
//! so they cover the actual layer composition (global rate-limit layer,
//! per-route auth layer, per-route rate-limit layer). They do NOT bypass
//! `auth_middleware` or insert `IdentityContext` directly — that would
//! invalidate the very ordering they are meant to prove.
//!
//! The tests run with `enforce_zk_proofs = false` so they can use raw
//! pseudonyms as Bearer tokens. Behaviour for `enforce_zk_proofs = true`
//! is identical from a rate-limiter perspective: the limiter sees the
//! `IdentityContext` regardless of how it was derived.

mod common;

use annex_types::{ChannelType, FederationScope, ServerPolicy};
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tower::ServiceExt;

const PSEUDO_A: &str = "alice-rl";
const PSEUDO_B: &str = "bob-rl";

/// Build a policy whose default rate limit is small enough to exhaust in a
/// test without flooding the limiter with noise.
fn tight_policy() -> ServerPolicy {
    let mut p = ServerPolicy::default();
    p.rate_limit.default_limit = 3;
    p
}

/// Set up a test app with a tight rate limit and two registered pseudonyms.
/// The pseudonyms exist in `platform_identities` so `auth_middleware` can
/// resolve them.
async fn setup_with_two_pseudonyms() -> axum::Router {
    let (app, pool) = common::setup_test_app_with_policy(tight_policy()).await;

    let conn = pool.get().unwrap();
    for pseudo in [PSEUDO_A, PSEUDO_B] {
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, can_moderate, active) \
             VALUES (1, ?1, 'HUMAN', 0, 1)",
            [pseudo],
        )
        .unwrap();
    }

    // Seed a channel that PSEUDO_A and PSEUDO_B are members of. The handler
    // we hit is `GET /api/channels/{channelId}/voice/status`, which only
    // checks authentication + membership — perfect for cheaply exercising
    // a protected route many times.
    let channel_id = "rl-test-channel";
    let channel_type_json = serde_json::to_string(&ChannelType::Text).unwrap();
    let scope_json = serde_json::to_string(&FederationScope::Local).unwrap();
    conn.execute(
        "INSERT INTO channels (server_id, channel_id, name, channel_type, topic, federation_scope) \
         VALUES (1, ?1, 'rl', ?2, 'rl', ?3)",
        rusqlite::params![channel_id, channel_type_json, scope_json],
    )
    .unwrap();
    for pseudo in [PSEUDO_A, PSEUDO_B] {
        conn.execute(
            "INSERT INTO channel_members (server_id, channel_id, pseudonym_id, role) \
             VALUES (1, ?1, ?2, 'MEMBER')",
            rusqlite::params![channel_id, pseudo],
        )
        .unwrap();
    }

    app
}

/// Construct a request hitting a protected route, authenticated as `pseudo`
/// and reported as originating from `ip`.
fn protected_request(pseudo: &str, ip: IpAddr) -> Request<Body> {
    let mut req = Request::builder()
        .method("GET")
        .uri("/api/channels/rl-test-channel/voice/status")
        .header("Authorization", format!("Bearer {pseudo}"))
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(ip, 0)));
    req
}

/// Construct an unauthenticated request hitting a public route.
fn public_request(ip: IpAddr) -> Request<Body> {
    let mut req = Request::builder()
        .method("GET")
        .uri("/health")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(ip, 0)));
    req
}

#[tokio::test]
async fn protected_requests_are_pseudonym_limited_not_ip_limited() {
    // Two pseudonyms behind the SAME source IP. With per-IP keying the
    // second pseudonym would inherit the first's exhausted budget; with
    // per-pseudonym keying each one gets its own bucket.
    let app = setup_with_two_pseudonyms().await;
    let shared_ip: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7));

    // Exhaust PSEUDO_A's bucket (limit=3 → 4th must be 429).
    for i in 0..3 {
        let resp = app
            .clone()
            .oneshot(protected_request(PSEUDO_A, shared_ip))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::OK,
            "PSEUDO_A request {i} should pass"
        );
    }
    let resp = app
        .clone()
        .oneshot(protected_request(PSEUDO_A, shared_ip))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "PSEUDO_A 4th request must hit per-pseudonym limit"
    );

    // Now PSEUDO_B from the SAME IP must NOT be limited — its bucket is
    // independent. If we were keying by IP this would also be 429.
    let resp = app
        .oneshot(protected_request(PSEUDO_B, shared_ip))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "PSEUDO_B from same IP must not consume PSEUDO_A's quota"
    );
}

#[tokio::test]
async fn same_pseudonym_across_ips_shares_quota() {
    // The same pseudonym from two different IPs must share its
    // per-pseudonym bucket — that's the whole point of identity-keyed
    // rate limiting. Moving to a new IP should NOT reset the count.
    let app = setup_with_two_pseudonyms().await;
    let ip_one: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 8));
    let ip_two: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9));

    // Spend 2 of the 3-quota from ip_one.
    for _ in 0..2 {
        let resp = app
            .clone()
            .oneshot(protected_request(PSEUDO_A, ip_one))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
    // 3rd request from a DIFFERENT IP must still go through (under limit).
    let resp = app
        .clone()
        .oneshot(protected_request(PSEUDO_A, ip_two))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 4th request — regardless of source IP — must hit per-pseudonym 429.
    let resp = app
        .oneshot(protected_request(PSEUDO_A, ip_one))
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "4th request (any IP) must hit per-pseudonym limit"
    );
}

#[tokio::test]
async fn unauthenticated_public_requests_are_ip_limited() {
    // Public routes (no auth_middleware) must still be rate-limited, and
    // since there is no IdentityContext, the limiter falls back to IP.
    // Two different IPs must each be allowed their own quota.
    let app = setup_with_two_pseudonyms().await;
    let ip_a: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10));
    let ip_b: IpAddr = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 11));

    // Exhaust ip_a's bucket on the public /health endpoint.
    for _ in 0..3 {
        let resp = app.clone().oneshot(public_request(ip_a)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
    let resp = app.clone().oneshot(public_request(ip_a)).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "4th /health from ip_a must hit per-IP limit"
    );

    // ip_b — independent bucket — must still be allowed.
    let resp = app.oneshot(public_request(ip_b)).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "different IP must not inherit exhausted IP's quota"
    );
}

#[tokio::test]
async fn unauthenticated_request_to_protected_route_is_rejected_before_consuming_quota() {
    // Sanity check: a missing Authorization header on a protected route
    // returns 401 from `auth_middleware`, NOT 429. The per-route rate
    // limit only fires after auth has run, and the global rate limit
    // (IP-keyed) only kicks in after the request has actually reached
    // the limiter — but a 401 from auth still consumes some budget at
    // the global IP layer. The relevant invariant for production is:
    // unauthenticated callers can NEVER drain another user's
    // per-pseudonym budget. This test confirms the auth gate sits
    // upstream of the per-pseudonym bucket.
    let app = setup_with_two_pseudonyms().await;
    let ip: IpAddr = IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7));

    let mut req = Request::builder()
        .method("GET")
        .uri("/api/channels/rl-test-channel/voice/status")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::new(ip, 0)));

    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "missing auth must return 401, not 429 (auth runs before the per-route rate limit)"
    );

    // PSEUDO_A's per-pseudonym bucket must NOT have been touched.
    for _ in 0..3 {
        let resp = app
            .clone()
            .oneshot(protected_request(PSEUDO_A, ip))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
