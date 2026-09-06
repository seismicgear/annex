//! `GET /api/link-preview` and `GET /api/link-preview/image` — the two
//! routes that make the server fetch a URL somebody else chose.
//!
//! `api_link_preview.rs` has fifteen unit tests and every one of them calls
//! `is_private_or_reserved` directly. That proves the predicate is right. It
//! cannot prove the routes ask it — and the routes are the whole exposure,
//! because these are the only endpoints where a caller's input becomes an
//! outbound request from inside the server's network.
//!
//! The image proxy is deliberately unauthenticated (a browser loading
//! `<img src>` sends no custom headers), which is a defensible trade and
//! also means it is reachable by anyone who can reach the server at all.
//! An unguarded fetch there is not a link-preview bug, it is a port scanner
//! and a cloud-metadata credential reader with a public URL.
//!
//! Every test here is offline by construction: each URL is rejected before
//! any DNS or TCP happens, so the suite does not depend on the network and
//! cannot pass merely because a fetch timed out.

mod common;

use annex_db::DbPool;
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use common::setup_test_app;
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

/// Requests a preview as an authenticated member.
async fn preview(app: &axum::Router, target: &str) -> StatusCode {
    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut req = Request::builder()
        .uri(format!(
            "/api/link-preview?url={}",
            urlencoding_encode(target)
        ))
        .method("GET")
        .header("X-Annex-Pseudonym", "alice")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    app.clone().oneshot(req).await.unwrap().status()
}

/// Requests the same target through the *unauthenticated* image proxy.
async fn proxy(app: &axum::Router, target: &str) -> StatusCode {
    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut req = Request::builder()
        .uri(format!(
            "/api/link-preview/image?url={}",
            urlencoding_encode(target)
        ))
        .method("GET")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));
    app.clone().oneshot(req).await.unwrap().status()
}

/// Percent-encodes a query-parameter value. Written out rather than pulled
/// in as a dependency so the encoding is visible: several of these targets
/// contain `@`, `[`, `]` and `:`, which are exactly the characters a URL
/// parser disagrees about, and a test that accidentally encoded them
/// differently from a browser would be testing the wrong string.
fn urlencoding_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Targets that must never produce an outbound request. Each is a way an
/// attacker has historically reached an internal service through a
/// well-meaning URL-fetching feature.
const BLOCKED: &[(&str, &str)] = &[
    ("http://127.0.0.1/admin", "IPv4 loopback"),
    (
        "http://localhost:3000/api/admin/members",
        "the server itself, by name",
    ),
    ("http://[::1]/", "IPv6 loopback"),
    (
        "http://169.254.169.254/latest/meta-data/",
        "AWS/Azure instance metadata",
    ),
    (
        "http://metadata.google.internal/computeMetadata/v1/",
        "GCP metadata",
    ),
    ("http://10.1.2.3/", "RFC1918 10/8"),
    ("http://192.168.0.1/", "RFC1918 192.168/16"),
    ("http://172.16.5.4/", "RFC1918 172.16/12"),
    ("http://100.64.0.1/", "CGNAT shared address space"),
    ("http://0.0.0.0/", "the unspecified address"),
    ("http://255.255.255.255/", "broadcast"),
    ("http://[fd00::1]/", "IPv6 unique-local"),
    ("http://[fe80::1]/", "IPv6 link-local"),
    ("http://[::ffff:127.0.0.1]/", "IPv4-mapped IPv6 loopback"),
    ("http://db.internal/", ".internal"),
    ("http://printer.local/", ".local (mDNS)"),
    ("file:///etc/passwd", "a non-http scheme"),
    (
        "gopher://127.0.0.1:6379/_INFO",
        "gopher, the classic Redis SSRF",
    ),
    ("ftp://127.0.0.1/", "ftp"),
    ("not a url at all", "an unparseable string"),
];

#[tokio::test]
async fn the_preview_route_refuses_every_internal_target() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    for (target, why) in BLOCKED {
        let status = preview(&app, target).await;
        assert!(
            status == StatusCode::FORBIDDEN || status == StatusCode::BAD_REQUEST,
            "{target} ({why}) was not refused — got {status}",
        );
    }
}

#[tokio::test]
async fn the_image_proxy_refuses_every_internal_target_too() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    // The proxy is the unauthenticated one. If the two handlers ever drift,
    // this is the half that matters, so it is asserted separately rather
    // than assumed to share the preview route's checks.
    for (target, why) in BLOCKED {
        let status = proxy(&app, target).await;
        assert!(
            status == StatusCode::FORBIDDEN || status == StatusCode::BAD_REQUEST,
            "{target} ({why}) was not refused by the public image proxy — got {status}",
        );
    }
}

#[tokio::test]
async fn a_hostname_that_only_looks_public_does_not_win() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    // `userinfo@host` — everything before the `@` is a username, so the real
    // host is the loopback address. A check that string-matches the start of
    // the URL, or that reads up to the first `/`, sees "example.com" and
    // waves it through.
    let status = preview(&app, "http://example.com@127.0.0.1/admin").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "userinfo before the host defeated the SSRF check",
    );
}

#[tokio::test]
async fn an_integer_encoded_loopback_address_is_still_loopback() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    // 2130706433 == 0x7F000001 == 127.0.0.1. The URL spec parses bare
    // integers as IPv4, so this reaches the same service while looking
    // nothing like an IP to a naive check.
    let status = preview(&app, "http://2130706433/").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the decimal form of 127.0.0.1 was allowed",
    );
}

#[tokio::test]
async fn an_octal_encoded_loopback_address_is_still_loopback() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    // 0177.0.0.1 — octal 0177 is 127.
    let status = preview(&app, "http://0177.0.0.1/").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "the octal form of 127.0.0.1 was allowed",
    );
}

#[tokio::test]
async fn case_does_not_smuggle_a_blocked_host_through() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    let status = preview(&app, "HTTP://LOCALHOST/").await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "an uppercase host bypassed the blocklist",
    );
}

#[tokio::test]
async fn surrounding_whitespace_does_not_smuggle_a_blocked_host_through() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    let status = preview(&app, "  http://127.0.0.1/  ").await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::BAD_REQUEST,
        "a padded URL was not refused — got {status}",
    );
}

// ── Bounds ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn an_empty_url_is_a_client_error() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    let status = preview(&app, "").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_over_long_url_is_refused_before_anything_is_fetched() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    let long = format!("https://example.com/{}", "a".repeat(2100));
    let status = preview(&app, &long).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a 2KB+ URL should be rejected on length, not sent upstream",
    );
}

#[tokio::test]
async fn a_request_with_no_url_parameter_is_a_client_error() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut req = Request::builder()
        .uri("/api/link-preview")
        .method("GET")
        .header("X-Annex-Pseudonym", "alice")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let status = app.oneshot(req).await.unwrap().status();
    assert!(
        status.is_client_error(),
        "a preview request with no url should be rejected: {status}",
    );
}

// ── Authentication ───────────────────────────────────────────────────────

#[tokio::test]
async fn the_preview_route_requires_an_identity() {
    let (app, pool) = setup_test_app().await;
    add_member(&pool, "alice");

    // Unlike the image proxy, the preview route has no reason to be open:
    // it is called by application code that already holds an identity.
    // Leaving it open would make the server a general-purpose URL fetcher
    // for anyone who can reach it.
    let addr: SocketAddr = "127.0.0.1:9000".parse().unwrap();
    let mut req = Request::builder()
        .uri("/api/link-preview?url=https%3A%2F%2Fexample.com%2F")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    req.extensions_mut().insert(ConnectInfo(addr));

    let status = app.oneshot(req).await.unwrap().status();
    assert_ne!(
        status,
        StatusCode::OK,
        "link preview served an anonymous caller",
    );
}
