//! Unit tests for `check_freshness` (the time gate) and
//! `message_envelope_hash` (the replay-ledger key).
//!
//! Receiver-side integration of the receipt ledger is covered by the
//! existing federation lifecycle tests once they are upgraded to send
//! v2 envelopes; that upgrade is queued behind the v2 prover landing
//! on the client side (see `docs/refactor/handoff-hardening-pass.md`).

use annex_federation::FederatedMessageEnvelope;
use annex_server::services::federation_service::{
    check_freshness, message_envelope_hash, message_signing_input, DeliveryMode, FreshnessRejection,
};
use chrono::TimeZone;

fn env(version: Option<&str>, created_at: &str) -> FederatedMessageEnvelope {
    FederatedMessageEnvelope {
        envelope_version: version.map(str::to_string),
        message_id: "msg-1".to_string(),
        channel_id: "chan-1".to_string(),
        content: "hello".to_string(),
        sender_pseudonym: "psn-1".to_string(),
        originating_server: "https://peer.example.com".to_string(),
        attestation_ref: "annex:server:v1:abc".to_string(),
        signature: String::new(),
        created_at: created_at.to_string(),
    }
}

#[test]
fn v1_signing_input_unchanged_from_legacy_format() {
    let e = env(None, "2026-05-12T00:00:00Z");
    let s = message_signing_input(&e);
    // Legacy 7-line format MUST NOT change; peers signing with the
    // unversioned input depend on it being byte-identical.
    let expected = "msg-1\nchan-1\nhello\npsn-1\nhttps://peer.example.com\nannex:server:v1:abc\n2026-05-12T00:00:00Z";
    assert_eq!(s, expected);
}

#[test]
fn v2_signing_input_prepends_version_line() {
    let e = env(Some("v2"), "2026-05-12T00:00:00Z");
    let s = message_signing_input(&e);
    let expected = "v2\nmsg-1\nchan-1\nhello\npsn-1\nhttps://peer.example.com\nannex:server:v1:abc\n2026-05-12T00:00:00Z";
    assert_eq!(s, expected);
}

#[test]
fn v1_and_v2_envelopes_have_distinct_hashes() {
    let v1 = env(None, "2026-05-12T00:00:00Z");
    let v2 = env(Some("v2"), "2026-05-12T00:00:00Z");
    assert_ne!(
        message_envelope_hash(&v1),
        message_envelope_hash(&v2),
        "downgrade attack would only succeed if v1/v2 hashed identically — they must not"
    );
}

#[test]
fn freshness_accepts_recent_live_envelope() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 12, 12, 0, 0).unwrap();
    let created_at = (now - chrono::Duration::seconds(60)).to_rfc3339();
    assert!(check_freshness(&created_at, now, 300, 60, DeliveryMode::Live).is_ok());
}

#[test]
fn freshness_rejects_stale_live_envelope() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 12, 12, 0, 0).unwrap();
    let created_at = (now - chrono::Duration::seconds(500)).to_rfc3339();
    assert_eq!(
        check_freshness(&created_at, now, 300, 60, DeliveryMode::Live),
        Err(FreshnessRejection::TooOld)
    );
}

#[test]
fn freshness_accepts_stale_envelope_via_catchup() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 12, 12, 0, 0).unwrap();
    let created_at = (now - chrono::Duration::seconds(86_400)).to_rfc3339();
    assert!(check_freshness(&created_at, now, 300, 60, DeliveryMode::Catchup).is_ok());
}

#[test]
fn freshness_rejects_future_envelope_outside_skew() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 12, 12, 0, 0).unwrap();
    let created_at = (now + chrono::Duration::seconds(120)).to_rfc3339();
    assert_eq!(
        check_freshness(&created_at, now, 300, 60, DeliveryMode::Live),
        Err(FreshnessRejection::TooFarInFuture)
    );
}

#[test]
fn freshness_rejects_unparseable_created_at() {
    let now = chrono::Utc.with_ymd_and_hms(2026, 5, 12, 12, 0, 0).unwrap();
    assert_eq!(
        check_freshness("not-a-timestamp", now, 300, 60, DeliveryMode::Live),
        Err(FreshnessRejection::Unparseable)
    );
}
