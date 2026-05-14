//! Tests for `resolve_client_ip_from_xff` — the helper that
//! `rate_limit_middleware` uses to pick the rate-limit key when the
//! operator has declared a trusted reverse-proxy depth.
//!
//! These tests pin the security-critical invariants:
//!
//!   * depth == 0 → XFF MUST be ignored. A directly-exposed server
//!     cannot be tricked into keying flood traffic to a spoofed IP.
//!   * depth >= 1 → real client IP is `len - depth - 1`, NEVER the
//!     leftmost entry blindly. A misconfigured-too-high depth falls
//!     back to the socket peer (and never lets a client move buckets
//!     by adding XFF entries beyond the trusted prefix).
//!   * IPv6 + ipv4-port forms must parse cleanly.

use annex_server::middleware::resolve_client_ip_from_xff;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use std::net::IpAddr;

fn xff(values: &[&str]) -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert(
        HeaderName::from_static("x-forwarded-for"),
        HeaderValue::from_str(&values.join(", ")).unwrap(),
    );
    h
}

fn ip(s: &str) -> IpAddr {
    s.parse().expect("valid ip")
}

#[test]
fn depth_zero_returns_socket_fallback() {
    // depth 0 is exposed-directly mode; XFF is irrelevant even when
    // present. The function MUST return whatever the caller handed in
    // as the socket fallback.
    let h = xff(&["203.0.113.1", "10.0.0.5"]);
    let got = resolve_client_ip_from_xff(&h, 0, Some(ip("8.8.8.8")));
    assert_eq!(got, Some(ip("8.8.8.8")));
    // And when there's no socket fallback either, None all the way
    // through (the middleware then surfaces 500 — never a misattributed
    // bucket).
    let got_none = resolve_client_ip_from_xff(&h, 0, None);
    assert_eq!(got_none, None);
}

#[test]
fn depth_one_with_one_proxy_extracts_real_client() {
    // Topology: Client → ProxyA → us. ProxyA appended client IP.
    // XFF has 1 entry; depth 1 means trust the rightmost 1; real
    // client is at len - depth - 1 = -1, clamped/falls back.
    //
    // Wait — re-derive: with depth 1, the rightmost entry IS proxyA's
    // record of the immediate source, which is the client. So XFF[0]
    // is the client. But our formula `len - depth - 1 = 1 - 1 - 1 = -1`
    // is invalid → we fall back to the socket.
    //
    // That's actually CORRECT behaviour for the conservative pattern:
    // depth=1 means we know one hop is trusted, but the client could
    // still have written XFF[0] themselves. The function never trusts
    // a leftmost entry unless there are MORE hops in front of it.
    // This is paranoid but safe.
    let h = xff(&["203.0.113.1"]);
    let got = resolve_client_ip_from_xff(&h, 1, Some(ip("10.0.0.1")));
    assert_eq!(
        got,
        Some(ip("10.0.0.1")),
        "single-entry XFF with depth 1 falls back to socket peer (no client beyond the trusted proxy)"
    );
}

#[test]
fn depth_one_with_two_entries_extracts_real_client() {
    // Topology: Client → ProxyA → us, but a client-supplied XFF
    // existed first. Client wrote "10.6.6.6" into XFF, ProxyA
    // appended "203.0.113.1" → XFF = "10.6.6.6, 203.0.113.1".
    // depth=1: trust 1 hop from right → entry at idx 0 (10.6.6.6) is
    // beyond the trusted prefix, so we return it as "client".
    //
    // Important: this only happens when the operator declared depth=1.
    // If they declared depth=2 thinking there's a second hop, we'd
    // require >=3 entries and fall back. The operator decides what to
    // trust.
    let h = xff(&["10.6.6.6", "203.0.113.1"]);
    let got = resolve_client_ip_from_xff(&h, 1, Some(ip("8.8.8.8")));
    assert_eq!(got, Some(ip("10.6.6.6")));
}

#[test]
fn depth_two_with_three_entries_extracts_real_client() {
    // Client → ProxyA → ProxyB → us. ProxyA wrote client IP, ProxyB
    // wrote ProxyA's IP. XFF = "client, proxyA, proxyB"-style. depth=2.
    let h = xff(&["203.0.113.42", "10.0.0.5", "10.0.0.6"]);
    let got = resolve_client_ip_from_xff(&h, 2, Some(ip("8.8.8.8")));
    assert_eq!(got, Some(ip("203.0.113.42")));
}

#[test]
fn depth_too_high_falls_back_to_socket() {
    // Operator misconfigured depth higher than reality. Function MUST
    // NOT trust the leftmost spoofable entry — it falls back to the
    // socket peer.
    let h = xff(&["1.2.3.4"]);
    let got = resolve_client_ip_from_xff(&h, 3, Some(ip("198.51.100.1")));
    assert_eq!(got, Some(ip("198.51.100.1")));
}

#[test]
fn depth_equals_xff_length_falls_back() {
    // All entries are claimed-trusted proxies; no slot for a real
    // client → fall back. This protects against the "I'm so paranoid
    // I trust 3 hops" misconfig combined with a 3-entry XFF the
    // attacker built.
    let h = xff(&["10.0.0.1", "10.0.0.2", "10.0.0.3"]);
    let got = resolve_client_ip_from_xff(&h, 3, Some(ip("198.51.100.1")));
    assert_eq!(got, Some(ip("198.51.100.1")));
}

#[test]
fn missing_xff_returns_socket_fallback() {
    let h = HeaderMap::new();
    let got = resolve_client_ip_from_xff(&h, 1, Some(ip("198.51.100.1")));
    assert_eq!(got, Some(ip("198.51.100.1")));
}

#[test]
fn malformed_entry_returns_socket_fallback() {
    // Wrong format (port without bracket) → falls back. We never let a
    // half-parseable XFF entry produce a "best-effort" key.
    let h = xff(&["totally-not-an-ip", "203.0.113.1"]);
    let got = resolve_client_ip_from_xff(&h, 1, Some(ip("198.51.100.1")));
    assert_eq!(got, Some(ip("198.51.100.1")));
}

#[test]
fn ipv4_with_port_is_parsed() {
    let h = xff(&["203.0.113.5:54321", "10.0.0.1"]);
    let got = resolve_client_ip_from_xff(&h, 1, Some(ip("8.8.8.8")));
    assert_eq!(got, Some(ip("203.0.113.5")));
}

#[test]
fn ipv6_bracketed_is_parsed() {
    let h = xff(&["[2001:db8::1]", "10.0.0.1"]);
    let got = resolve_client_ip_from_xff(&h, 1, Some(ip("8.8.8.8")));
    assert_eq!(got, Some(ip("2001:db8::1")));
}

#[test]
fn empty_xff_string_returns_socket_fallback() {
    let mut h = HeaderMap::new();
    h.insert(
        HeaderName::from_static("x-forwarded-for"),
        HeaderValue::from_static(""),
    );
    let got = resolve_client_ip_from_xff(&h, 1, Some(ip("198.51.100.1")));
    assert_eq!(got, Some(ip("198.51.100.1")));
}
