//! The Rust half of a two-implementation contract.
//!
//! `api/signal.js` is a second implementation of the signaling wire format,
//! written in a different language and deployed separately. For the whole life
//! of `FederationTransport` the two disagreed about what gets signed — Rust
//! interpolated `rendezvous_tag` as a fourth field of the canonical string and
//! JavaScript did not — so a signature produced here could never verify there.
//! Every envelope the transport sent would have been a 401. Nothing failed,
//! because nothing instantiated the transport; a unit test on either side
//! passed happily against its own idea of the format.
//!
//! That is the defect class a shared vector file exists to catch: each side is
//! internally consistent and the pair is broken. `api/canonical-vectors.json`
//! and `api/rendezvous-vectors.json` are read by this file and by
//! `api/signal.test.mjs`, so a change to either implementation that moves a
//! byte fails on both sides at once.
//!
//! Regenerate after a deliberate format change:
//!
//! ```text
//! cargo test -p annex-federation --test canonical_vectors -- --ignored emit
//! ```
//!
//! and then run the suite normally, plus `node --test api/signal.test.mjs`.

use annex_federation::metadata::rendezvous_tag_for;
use annex_federation::signal::{DrainAuth, SignalingPayload, FORMAT_V1, FORMAT_V2};
use ed25519_dalek::{Signer, SigningKey, VerifyingKey};
use serde_json::{json, Value};
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root should resolve")
}

/// Deterministic keys. A vector file generated from random keys would change
/// every time it was regenerated, which makes a real format change
/// indistinguishable from a re-run in review.
fn key(seed_byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed_byte; 32])
}

fn hex32(vk: &VerifyingKey) -> String {
    vk.to_bytes().iter().map(|b| format!("{b:02x}")).collect()
}

/// The envelopes the vectors are built from. Values are fixed and
/// unremarkable on purpose; what is under test is the concatenation, not the
/// content.
fn sample_payloads() -> Vec<SignalingPayload> {
    let sender = key(0x11);
    let recipient = key(0x22);
    let from_pubkey_hex = hex32(&sender.verifying_key());

    vec![
        SignalingPayload {
            format_version: FORMAT_V1,
            from_server_slug: "0123456789ab".to_string(),
            to_server_slug: "fedcba987654".to_string(),
            rendezvous_tag: String::new(),
            session_id: "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9".to_string(),
            sdp_type: "offer".to_string(),
            sdp: "c2VhbGVkLXNkcC1ieXRlcw".to_string(),
            sent_at_ms: 1_767_225_600_000,
            from_pubkey_hex: from_pubkey_hex.clone(),
            vrp_signature: String::new(),
        },
        SignalingPayload {
            format_version: FORMAT_V2,
            from_server_slug: String::new(),
            to_server_slug: String::new(),
            rendezvous_tag: rendezvous_tag_for(&recipient.verifying_key(), 497_000),
            session_id: "0a1b2c3d-4e5f-6071-8293-a4b5c6d7e8f9".to_string(),
            sdp_type: "answer".to_string(),
            sdp: "c2VhbGVkLXNkcC1ieXRlcw".to_string(),
            sent_at_ms: 1_767_225_600_000,
            from_pubkey_hex,
            vrp_signature: String::new(),
        },
    ]
}

fn sign(sk: &SigningKey, input: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(sk.sign(input.as_bytes()).to_bytes())
}

fn canonical_vectors() -> Value {
    let sender = key(0x11);
    let envelopes: Vec<Value> = sample_payloads()
        .iter()
        .map(|p| {
            json!({
                "envelope": {
                    "format_version": p.format_version,
                    "from_server_slug": p.from_server_slug,
                    "to_server_slug": p.to_server_slug,
                    "rendezvous_tag": p.rendezvous_tag,
                    "session_id": p.session_id,
                    "sdp_type": p.sdp_type,
                    "sdp": p.sdp,
                    "sent_at_ms": p.sent_at_ms,
                    "from_pubkey_hex": p.from_pubkey_hex,
                    // Produced HERE, by ed25519-dalek, and verified THERE by
                    // node:crypto. A matching canonical string proves the two
                    // sides agree about what to hash; only a real signature
                    // crossing the boundary proves the whole verification path
                    // agrees — encoding, key format, and all.
                    "vrp_signature": sign(&sender, &p.canonical_signing_input()),
                },
                "canonical": p.canonical_signing_input(),
                "queue_key": p.queue_key(),
            })
        })
        .collect();

    // The drainer signs with its OWN key, and the tag it drains is its own
    // rendezvous address — so the drain vector must use the recipient key,
    // not the sender's.
    let drainer = key(0x22);
    let drain_tag = rendezvous_tag_for(&drainer.verifying_key(), 497_000);
    let drain_ts = 1_767_225_600_000i64;
    let drain_canonical = DrainAuth::canonical_signing_input(&drain_tag, drain_ts);
    json!({
        "_comment": "Generated by crates/annex-federation/tests/canonical_vectors.rs \
                     (cargo test -p annex-federation --test canonical_vectors -- --ignored emit). \
                     Read by that test and by api/signal.test.mjs so the Rust signer and the \
                     JavaScript verifier cannot drift apart.",
        "envelopes": envelopes,
        "drain": {
            "tag": drain_tag,
            "bucket": 497_000,
            "drain_pubkey_hex": hex32(&drainer.verifying_key()),
            "timestamp_ms": drain_ts,
            "canonical": drain_canonical,
            "signature": sign(&drainer, &drain_canonical),
        },
    })
}

fn rendezvous_vectors() -> Value {
    let tags: Vec<Value> = [0x22u8, 0x33, 0x44]
        .iter()
        .flat_map(|seed| {
            let vk = key(*seed).verifying_key();
            let pubkey_hex = hex32(&vk);
            [0u64, 1, 497_000, 4_294_967_296].into_iter().map(move |b| {
                json!({
                    "ed25519_pubkey_hex": pubkey_hex,
                    "bucket": b,
                    "tag": rendezvous_tag_for(&vk, b),
                })
            })
        })
        .collect();

    json!({
        "_comment": "Generated by crates/annex-federation/tests/canonical_vectors.rs. \
                     Pins the Ed25519 -> X25519 -> rendezvous-tag derivation across the Rust \
                     transport and api/signal.js, which recomputes a tag to authorise a drain.",
        "bucket_seconds": 3600,
        "tag_domain": "annex-rendezvous-tag-v1",
        "tags": tags,
    })
}

fn read(name: &str) -> Value {
    let path = repo_root().join("api").join(name);
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{} should exist ({e}) — regenerate with --ignored emit",
            path.display()
        )
    });
    serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} is not valid JSON: {e}", path.display()))
}

#[test]
fn canonical_vectors_match_the_committed_file() {
    assert_eq!(
        read("canonical-vectors.json"),
        canonical_vectors(),
        "the canonical signing input changed. If that was deliberate, regenerate the vectors \
         AND update canonicalEnvelope in api/signal.js — a relay that disagrees rejects every \
         envelope with a 401 and the only symptom is federation silently never connecting."
    );
}

#[test]
fn rendezvous_vectors_match_the_committed_file() {
    assert_eq!(
        read("rendezvous-vectors.json"),
        rendezvous_vectors(),
        "the rendezvous tag derivation changed. api/signal.js recomputes these to decide who \
         may drain a queue, so a drift here means either every drain is refused or — worse — \
         the wrong peer is allowed to drain."
    );
}

/// The v1 and v2 canonical strings must never be producible from each other.
#[test]
fn the_two_formats_cannot_collide() {
    let mut v1 = sample_payloads()[0].clone();
    // The adversarial case: a v1 envelope that tries to look like a v2 one by
    // putting the version literal where the leading field goes.
    v1.from_server_slug = "v2".to_string();
    v1.to_server_slug = sample_payloads()[1].rendezvous_tag.clone();
    let v2 = sample_payloads()[1].clone();
    assert_ne!(
        v1.canonical_signing_input(),
        v2.canonical_signing_input(),
        "a v1 envelope must not be able to canonicalise as a v2 one — otherwise a signature \
         over a slug-addressed envelope authorises a tag-addressed one"
    );
}

/// An envelope with no `format_version` on the wire is v1. Any other answer
/// signs out every peer built before rendezvous addressing existed.
#[test]
fn a_missing_format_version_reads_as_v1() {
    let raw = r#"{
        "from_server_slug": "0123456789ab",
        "to_server_slug": "fedcba987654",
        "session_id": "s",
        "sdp_type": "offer",
        "sdp": "x",
        "sent_at_ms": 1,
        "vrp_signature": ""
    }"#;
    let parsed: SignalingPayload = serde_json::from_str(raw).expect("legacy envelope should parse");
    assert_eq!(parsed.format_version, FORMAT_V1);
    assert_eq!(parsed.queue_key(), "fedcba987654");
    assert!(!parsed.canonical_signing_input().starts_with("v2|"));
}

#[test]
#[ignore = "generator: run with --ignored emit after a deliberate format change"]
fn emit() {
    let api = repo_root().join("api");
    for (name, value) in [
        ("canonical-vectors.json", canonical_vectors()),
        ("rendezvous-vectors.json", rendezvous_vectors()),
    ] {
        let path = api.join(name);
        let mut out = serde_json::to_string_pretty(&value).expect("vectors should serialise");
        out.push('\n');
        std::fs::write(&path, out).unwrap_or_else(|e| panic!("writing {}: {e}", path.display()));
        eprintln!("wrote {}", path.display());
    }
}
