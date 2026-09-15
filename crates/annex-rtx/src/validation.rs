//! Validation and transfer scope enforcement for RTX bundles.
//!
//! This module provides the logic for enforcing VRP transfer scope on
//! reflection summary bundles, checking for redacted topics, and
//! validating bundle structure before publish or delivery.

use crate::error::RtxError;
use crate::types::{BundleProvenance, ReflectionSummaryBundle};
use annex_vrp::VrpTransferScope;

/// Maximum length of `summary` text. Must be small enough that local
/// `messages`-table broadcasts and federation relay don't choke on a
/// single bundle. Mirrors the federation-message cap from
/// `annex_server::services::federation_service::FEDERATION_MAX_MESSAGE_CONTENT_LEN`.
pub const MAX_SUMMARY_BYTES: usize = 65_536;

/// Maximum length of `reasoning_chain`. Allowed to be larger than
/// `summary` since chain-of-thought outputs are inherently more verbose,
/// but still bounded so a single bundle cannot dominate the global 2 MiB
/// body cap.
pub const MAX_REASONING_CHAIN_BYTES: usize = 262_144;

/// Maximum number of caveat entries.
pub const MAX_CAVEATS: usize = 16;

/// Maximum length of any single caveat string.
pub const MAX_CAVEAT_BYTES: usize = 4_096;

/// Maximum number of domain tag entries.
pub const MAX_DOMAIN_TAGS: usize = 32;

/// Maximum length of any single domain tag string.
pub const MAX_DOMAIN_TAG_BYTES: usize = 64;

/// Maximum length of identifier-like fields (`bundle_id`,
/// `source_pseudonym`, `source_server`, `signature`, `vrp_handshake_ref`).
/// All of these are short identifiers / URLs / hex strings in practice.
pub const MAX_IDENTIFIER_BYTES: usize = 512;

/// Enforces transfer scope on a bundle, stripping restricted fields.
///
/// - `FullKnowledgeBundle`: returns the bundle unchanged.
/// - `ReflectionSummariesOnly`: strips `reasoning_chain`.
/// - `NoTransfer`: returns an error; the bundle cannot be transferred.
///
/// This function returns a new bundle rather than mutating in place,
/// preserving the original for logging and audit.
pub fn enforce_transfer_scope(
    bundle: &ReflectionSummaryBundle,
    scope: VrpTransferScope,
) -> Result<ReflectionSummaryBundle, RtxError> {
    match scope {
        VrpTransferScope::NoTransfer => Err(RtxError::TransferDenied(
            "transfer scope is NoTransfer".to_string(),
        )),
        VrpTransferScope::ReflectionSummariesOnly => {
            let mut scoped = bundle.clone();
            scoped.reasoning_chain = None;
            Ok(scoped)
        }
        VrpTransferScope::FullKnowledgeBundle => Ok(bundle.clone()),
    }
}

/// Checks whether a bundle attempts to share a redacted topic.
///
/// A topic is considered shared if it appears either as one of the bundle's
/// self-asserted `domain_tags` **or** as a whole word anywhere in the
/// free-text content (`summary`, `reasoning_chain`, `caveats`). Scanning the
/// content — not just the tags — is what makes redaction enforceable: a sender
/// cannot launder a prohibited topic into prose while tagging the bundle
/// `["general"]` (or leaving `domain_tags` empty).
///
/// Matching is case-insensitive and word-bounded (so a redacted topic
/// `"finance"` does not match `"refinanced"`). Comparison is ASCII-case-folded;
/// topics are short labels in practice.
///
/// Redacted topics represent knowledge domains the agent is prohibited from
/// sharing per its VRP agreement.
pub fn check_redacted_topics(
    bundle: &ReflectionSummaryBundle,
    redacted_topics: &[String],
) -> Result<(), RtxError> {
    if redacted_topics.is_empty() {
        return Ok(());
    }

    // 1. Self-asserted domain tags (case-insensitive exact match).
    for tag in &bundle.domain_tags {
        if redacted_topics.iter().any(|t| t.eq_ignore_ascii_case(tag)) {
            return Err(RtxError::RedactedTopic(tag.clone()));
        }
    }

    // 2. Free-text content — defeats tag-laundering.
    let mut haystacks: Vec<&str> = vec![bundle.summary.as_str()];
    if let Some(rc) = bundle.reasoning_chain.as_deref() {
        haystacks.push(rc);
    }
    for caveat in &bundle.caveats {
        haystacks.push(caveat.as_str());
    }
    for topic in redacted_topics {
        let needle = topic.trim();
        if needle.is_empty() {
            continue;
        }
        if haystacks.iter().any(|hay| contains_word_ci(hay, needle)) {
            return Err(RtxError::RedactedTopic(topic.clone()));
        }
    }

    Ok(())
}

/// Whole-word, ASCII-case-insensitive search. A match must be bounded by a
/// non-alphanumeric character (or string edge) on both sides, so `"finance"`
/// matches `"in finance,"` but not `"refinanced"`.
fn contains_word_ci(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return false;
    }
    let hay = haystack.to_ascii_lowercase();
    let need = needle.to_ascii_lowercase();
    let bytes = hay.as_bytes();
    let mut start = 0;
    while let Some(pos) = hay[start..].find(&need) {
        let i = start + pos;
        let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
        let after = i + need.len();
        let after_ok = after >= bytes.len() || !bytes[after].is_ascii_alphanumeric();
        if before_ok && after_ok {
            return true;
        }
        start = i + need.len();
        if start >= hay.len() {
            break;
        }
    }
    false
}

/// Validates that a bundle has all required fields populated AND that
/// every variable-length field is within sane size bounds.
///
/// This performs structural validation only — it does not verify the
/// cryptographic signature here (that requires the sender's public key). The
/// per-agent **author** signature IS verified in the publish path
/// (`annex_server::services::rtx_service::verify_bundle_author_signature`)
/// against the agent's `signing_pubkey` captured at VRP handshake, over
/// [`author_signing_payload`], whenever the agent has advertised a key.
///
/// Size bounds are enforced consistently across the publish path
/// (`RtxService::publish_bundle`) and the federation receive path
/// (`FederationService::receive_federated_rtx`) so a federated peer
/// cannot push pathologically large bundles past the local 64 KiB
/// message cap and into the database / WS broadcast / relay fan-out.
pub fn validate_bundle_structure(bundle: &ReflectionSummaryBundle) -> Result<(), RtxError> {
    if bundle.bundle_id.is_empty() {
        return Err(RtxError::InvalidBundle("bundle_id is empty".to_string()));
    }
    if bundle.bundle_id.len() > MAX_IDENTIFIER_BYTES {
        return Err(RtxError::InvalidBundle(format!(
            "bundle_id exceeds maximum length of {MAX_IDENTIFIER_BYTES} bytes"
        )));
    }
    if bundle.source_pseudonym.is_empty() {
        return Err(RtxError::InvalidBundle(
            "source_pseudonym is empty".to_string(),
        ));
    }
    if bundle.source_pseudonym.len() > MAX_IDENTIFIER_BYTES {
        return Err(RtxError::InvalidBundle(format!(
            "source_pseudonym exceeds maximum length of {MAX_IDENTIFIER_BYTES} bytes"
        )));
    }
    if bundle.source_server.is_empty() {
        return Err(RtxError::InvalidBundle(
            "source_server is empty".to_string(),
        ));
    }
    if bundle.source_server.len() > MAX_IDENTIFIER_BYTES {
        return Err(RtxError::InvalidBundle(format!(
            "source_server exceeds maximum length of {MAX_IDENTIFIER_BYTES} bytes"
        )));
    }
    if bundle.summary.is_empty() {
        return Err(RtxError::InvalidBundle("summary is empty".to_string()));
    }
    if bundle.summary.len() > MAX_SUMMARY_BYTES {
        return Err(RtxError::InvalidBundle(format!(
            "summary exceeds maximum length of {MAX_SUMMARY_BYTES} bytes"
        )));
    }
    if let Some(ref chain) = bundle.reasoning_chain {
        if chain.len() > MAX_REASONING_CHAIN_BYTES {
            return Err(RtxError::InvalidBundle(format!(
                "reasoning_chain exceeds maximum length of {MAX_REASONING_CHAIN_BYTES} bytes"
            )));
        }
    }
    if bundle.signature.is_empty() {
        return Err(RtxError::InvalidBundle("signature is empty".to_string()));
    }
    if bundle.signature.len() > MAX_IDENTIFIER_BYTES {
        return Err(RtxError::InvalidBundle(format!(
            "signature exceeds maximum length of {MAX_IDENTIFIER_BYTES} bytes"
        )));
    }
    if bundle.vrp_handshake_ref.is_empty() {
        return Err(RtxError::InvalidBundle(
            "vrp_handshake_ref is empty".to_string(),
        ));
    }
    if bundle.vrp_handshake_ref.len() > MAX_IDENTIFIER_BYTES {
        return Err(RtxError::InvalidBundle(format!(
            "vrp_handshake_ref exceeds maximum length of {MAX_IDENTIFIER_BYTES} bytes"
        )));
    }
    if bundle.created_at == 0 {
        return Err(RtxError::InvalidBundle(
            "created_at must be non-zero".to_string(),
        ));
    }
    if bundle.domain_tags.len() > MAX_DOMAIN_TAGS {
        return Err(RtxError::InvalidBundle(format!(
            "domain_tags has {} entries (max {MAX_DOMAIN_TAGS})",
            bundle.domain_tags.len()
        )));
    }
    for tag in &bundle.domain_tags {
        if tag.len() > MAX_DOMAIN_TAG_BYTES {
            return Err(RtxError::InvalidBundle(format!(
                "domain_tag exceeds maximum length of {MAX_DOMAIN_TAG_BYTES} bytes"
            )));
        }
    }
    if bundle.caveats.len() > MAX_CAVEATS {
        return Err(RtxError::InvalidBundle(format!(
            "caveats has {} entries (max {MAX_CAVEATS})",
            bundle.caveats.len()
        )));
    }
    for caveat in &bundle.caveats {
        if caveat.len() > MAX_CAVEAT_BYTES {
            return Err(RtxError::InvalidBundle(format!(
                "caveat exceeds maximum length of {MAX_CAVEAT_BYTES} bytes"
            )));
        }
    }
    Ok(())
}

/// Constructs the signing payload for a bundle.
///
/// The signed message is the newline-delimited concatenation of:
/// `bundle_id\nsource_pseudonym\nsource_server\nsummary\ncreated_at`.
///
/// Fields are separated by newline (`\n`) to prevent ambiguity from
/// field value concatenation (e.g., `id="ab" + pseudo="cd"` vs `id="abcd"`).
///
/// Callers should SHA256-hash this payload and sign the hash with Ed25519.
///
/// NOTE: this legacy payload binds only metadata + summary. For the per-agent
/// **author** signature that must resist content tampering, use
/// [`author_signing_payload`], which binds every content field.
pub fn bundle_signing_payload(bundle: &ReflectionSummaryBundle) -> String {
    format!(
        "{}\n{}\n{}\n{}\n{}",
        bundle.bundle_id,
        bundle.source_pseudonym,
        bundle.source_server,
        bundle.summary,
        bundle.created_at
    )
}

/// Canonical author-signature payload binding **every content field** of a
/// bundle (everything except the `signature` field itself).
///
/// This is what the producing agent signs with its Ed25519 key and what the
/// server verifies against the agent's `signing_pubkey` (captured at VRP
/// handshake). Unlike [`bundle_signing_payload`] — which covers only metadata
/// and the summary — this binds `domain_tags`, `reasoning_chain`, `caveats`,
/// and `vrp_handshake_ref` too, so an agent (or a relay) cannot alter any
/// content field without invalidating the author signature. (This is the
/// per-agent author-authenticity half of AUDIT P4-FED-1; the relay/content
/// hash closed the in-transit-rewrite half.)
///
/// Encoding: each field is emitted as `len(bytes) || ':' || bytes || '\n'`
/// (length-prefixed) so no field value can be confused with a delimiter or
/// with an adjacent field — domain `annex/rtx/author-sig/v1`. Vec fields emit
/// their element count, then each element length-prefixed. `reasoning_chain`
/// emits a leading `0`/`1` presence byte so `None` and `Some("")` differ.
///
/// Callers SHA-256 this payload and sign/verify the digest with Ed25519.
pub fn author_signing_payload(bundle: &ReflectionSummaryBundle) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    out.push_str("annex/rtx/author-sig/v1\n");
    let field = |s: &str, buf: &mut String| {
        let _ = writeln!(buf, "{}:{}", s.len(), s);
    };
    field(&bundle.bundle_id, &mut out);
    field(&bundle.source_pseudonym, &mut out);
    field(&bundle.source_server, &mut out);
    let _ = writeln!(out, "{}", bundle.created_at);
    field(&bundle.vrp_handshake_ref, &mut out);
    field(&bundle.summary, &mut out);
    match &bundle.reasoning_chain {
        Some(rc) => {
            out.push_str("1\n");
            field(rc, &mut out);
        }
        None => out.push_str("0\n"),
    }
    let _ = writeln!(out, "tags:{}", bundle.domain_tags.len());
    for t in &bundle.domain_tags {
        field(t, &mut out);
    }
    let _ = writeln!(out, "caveats:{}", bundle.caveats.len());
    for c in &bundle.caveats {
        field(c, &mut out);
    }
    out
}

// ── Multi-hop relay: the provenance chain ──────────────────────────────────
//
// Single-hop RTX needed none of this. `relay_path` was a `Vec<String>` and the
// envelope carried one signature from the one relayer, which was enough because
// there was only ever one. Multi-hop breaks both assumptions at once: a relayer
// is no longer the author of the provenance it forwards, and the receiver has no
// relationship with the servers in the middle.

/// The absolute bound on relay depth, independent of what any origin asks for.
///
/// 5, matching `MAX_REDIRECT_HOPS` in `api_link_preview.rs` — the only hop-limit
/// precedent in this repository. The number is chosen, not measured: no Annex
/// federation exists yet to measure a diameter on. It is a ceiling rather than a
/// target, and the per-bundle budget defaults lower (`rtx_max_hops`, 3), because
/// CLAUDE.md's defect class 4 cuts both ways here — a budget larger than the
/// real federation diameter is decorative, and one smaller drops legitimate
/// traffic with nothing user-visible to say so.
pub const RTX_HOP_CEILING: usize = 5;

/// A content digest over everything that SURVIVES transfer-scope enforcement.
///
/// The origin cannot sign the hash a receiver computes. `enforce_transfer_scope`
/// strips the reasoning chain for a `ReflectionSummariesOnly` peer, so the bytes
/// leaving hop 2 are legitimately not the bytes that arrived — and any
/// origin-level signature over the full content would fail at every downstream
/// hop, for the correct behaviour of the system.
///
/// So this omits `reasoning_chain` entirely, and the origin commits to that part
/// separately (`OriginAttestation::reasoning_commitment`). A relayer may REMOVE
/// the chain; it cannot add or alter one.
pub fn scope_invariant_content_digest(bundle: &ReflectionSummaryBundle) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    let mut absorb = |bytes: &[u8]| {
        h.update((bytes.len() as u64).to_le_bytes());
        h.update(bytes);
    };
    absorb(b"annex/rtx/scope-invariant/v1");
    absorb(bundle.bundle_id.as_bytes());
    absorb(bundle.source_pseudonym.as_bytes());
    absorb(bundle.source_server.as_bytes());
    absorb(bundle.summary.as_bytes());
    absorb(bundle.created_at.to_string().as_bytes());
    absorb(bundle.signature.as_bytes());
    absorb(bundle.vrp_handshake_ref.as_bytes());
    // Deliberately NOT domain_tags or caveats: `enforce_transfer_scope` filters
    // both. Only `check_redacted_topics` can reject on tags, and it runs on the
    // bundle as received.
    hex::encode(h.finalize())
}

/// `SHA-256` of a reasoning chain, or of the empty string when there is none.
///
/// `None` and `Some("")` hash the same on purpose: the distinction is not
/// meaningful to a receiver deciding whether the chain it holds is the one the
/// origin wrote, and collapsing it means a relayer that normalises an empty
/// string to `None` does not break the commitment.
pub fn reasoning_commitment(chain: Option<&str>) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(chain.unwrap_or("").as_bytes()))
}

/// What the origin server signs. Domain `annex/rtx/origin-attestation/v1`.
///
/// Length-prefixed per field in the same style as [`author_signing_payload`],
/// and deliberately not `join`-ed: a `|`-joined path lets one field's content
/// impersonate a field boundary, which is the bug the author payload's encoding
/// was written to avoid and which the old `rtx_relay_signing_payload` still has.
#[allow(clippy::too_many_arguments)] // every argument is a distinct signed field; folding them into a struct would hide what the signature covers
pub fn origin_attestation_payload(
    bundle_id: &str,
    origin_server: &str,
    source_pseudonym: &str,
    created_at: u128,
    vrp_handshake_ref: &str,
    invariant_digest: &str,
    reasoning_commitment: &str,
    max_hops: u8,
) -> String {
    use std::fmt::Write;
    let mut out = String::from(
        "annex/rtx/origin-attestation/v1
",
    );
    let field = |s: &str, buf: &mut String| {
        let _ = writeln!(buf, "{}:{}", s.len(), s);
    };
    field(bundle_id, &mut out);
    field(origin_server, &mut out);
    field(source_pseudonym, &mut out);
    let _ = writeln!(out, "{created_at}");
    field(vrp_handshake_ref, &mut out);
    field(invariant_digest, &mut out);
    field(reasoning_commitment, &mut out);
    let _ = writeln!(out, "{max_hops}");
    out
}

/// What one relaying server signs for its own hop. Domain `annex/rtx/hop/v1`.
///
/// `prev_chain_digest` is `""` for hop 0 and otherwise
/// `SHA-256(relay_hop_payload of hop i-1)`. That single field is what makes the
/// chain a chain: without it each hop signature stands alone, and a relayer can
/// present any subset of hops in any order, each individually valid.
///
/// `next_peer` is bound too, so a hop's signature authorises forwarding to ONE
/// destination. A peer cannot take an envelope addressed to it and re-present
/// the same hop signature as though the upstream had sent it elsewhere.
#[allow(clippy::too_many_arguments)] // every argument is a distinct signed field; a struct here would hide what is covered
pub fn relay_hop_payload(
    bundle_id: &str,
    origin_server: &str,
    origin_signature: &str,
    hop_index: usize,
    hop_server: &str,
    next_peer: &str,
    content_hash: &str,
    prev_chain_digest: &str,
) -> String {
    use std::fmt::Write;
    let mut out = String::from("annex/rtx/hop/v1\n");
    let field = |s: &str, buf: &mut String| {
        let _ = writeln!(buf, "{}:{}", s.len(), s);
    };
    field(bundle_id, &mut out);
    field(origin_server, &mut out);
    field(origin_signature, &mut out);
    let _ = writeln!(out, "{hop_index}");
    field(hop_server, &mut out);
    field(next_peer, &mut out);
    field(content_hash, &mut out);
    field(prev_chain_digest, &mut out);
    out
}

/// `SHA-256` of a hop payload, hex — the value the NEXT hop chains onto.
pub fn chain_digest(payload: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(payload.as_bytes()))
}

/// The payload each hop in a chain should have signed, in order.
///
/// Both halves of the protocol need the same answer and must not derive it
/// independently: the relayer needs the last hop's payload to chain its own
/// signature onto, and the receiver needs all of them to verify. A second
/// implementation of this loop is a second chance to disagree about what was
/// signed, and a disagreement here reads as a forged chain.
///
/// `holder_url` is the server currently holding the envelope — the receiver's own
/// public URL when verifying, and the relayer's own when appending. It is the
/// `next_peer` of the LAST hop, because that hop forwarded the bundle to whoever
/// is now holding it. Every earlier hop's `next_peer` is the hop after it.
pub fn hop_payloads(
    provenance: &BundleProvenance,
    origin_signature: &str,
    holder_url: &str,
) -> Vec<String> {
    let mut payloads: Vec<String> = Vec::with_capacity(provenance.hops.len());
    let mut prev_digest = String::new();
    for (i, hop) in provenance.hops.iter().enumerate() {
        let next_peer = provenance
            .hops
            .get(i + 1)
            .map(|h| h.server.as_str())
            .unwrap_or(holder_url);
        let payload = relay_hop_payload(
            &provenance.bundle_id,
            &provenance.origin_server,
            origin_signature,
            i,
            &hop.server,
            next_peer,
            &hop.content_hash,
            &prev_digest,
        );
        prev_digest = chain_digest(&payload);
        payloads.push(payload);
    }
    payloads
}

/// Structural checks on a provenance chain, before any Ed25519 work.
///
/// Called FIRST on receive. A 500-hop envelope from an unauthenticated caller
/// should cost a bounds check, not 500 signature verifications —
/// `/api/federation/rtx` has no auth middleware in front of it, so the cheap
/// rejection is the one that matters.
pub fn validate_provenance_structure(provenance: &BundleProvenance) -> Result<(), RtxError> {
    if provenance.origin_server.is_empty() {
        return Err(RtxError::InvalidBundle(
            "provenance names no origin server".to_string(),
        ));
    }
    if provenance.origin_server.len() > MAX_IDENTIFIER_BYTES {
        return Err(RtxError::InvalidBundle(format!(
            "origin_server exceeds maximum length of {MAX_IDENTIFIER_BYTES} bytes"
        )));
    }
    if provenance.hops.len() > RTX_HOP_CEILING {
        return Err(RtxError::InvalidBundle(format!(
            "relay chain has {} hops, over the protocol ceiling of {RTX_HOP_CEILING}",
            provenance.hops.len()
        )));
    }
    let mut seen: Vec<&str> = Vec::with_capacity(provenance.hops.len());
    for hop in &provenance.hops {
        if hop.server.is_empty() {
            return Err(RtxError::InvalidBundle(
                "a relay hop names no server".to_string(),
            ));
        }
        for (label, value) in [
            ("hop server", &hop.server),
            ("hop content_hash", &hop.content_hash),
            ("hop signature", &hop.signature),
        ] {
            if value.len() > MAX_IDENTIFIER_BYTES {
                return Err(RtxError::InvalidBundle(format!(
                    "{label} exceeds maximum length of {MAX_IDENTIFIER_BYTES} bytes"
                )));
            }
        }
        // A repeated server is a cycle, and it is cheaper to see it here than
        // to discover it when the bundle comes back around.
        if seen.contains(&hop.server.as_str()) {
            return Err(RtxError::InvalidBundle(format!(
                "server {} appears twice in the relay chain",
                hop.server
            )));
        }
        seen.push(&hop.server);
    }
    Ok(())
}

#[cfg(test)]
mod hop_chain_tests {
    use super::*;
    use crate::types::RelayHop;

    fn bundle_with(reasoning: Option<&str>, tags: &[&str]) -> ReflectionSummaryBundle {
        ReflectionSummaryBundle {
            bundle_id: "b-hop".into(),
            source_pseudonym: "psn-a".into(),
            source_server: "https://a.example".into(),
            summary: "a summary".into(),
            reasoning_chain: reasoning.map(|r| r.to_string()),
            domain_tags: tags.iter().map(|t| t.to_string()).collect(),
            caveats: vec!["c1".into()],
            created_at: 1_700_000_000_000,
            signature: "aa".repeat(64),
            vrp_handshake_ref: "1:2:3".into(),
        }
    }

    #[test]
    fn the_invariant_digest_survives_stripping_the_reasoning_chain() {
        // The whole reason this digest exists. A `ReflectionSummariesOnly` peer
        // receives the bundle without its reasoning chain, and the origin's
        // signature has to still verify there.
        let full = bundle_with(Some("because of X, then Y"), &["ai-safety"]);
        let stripped = ReflectionSummaryBundle {
            reasoning_chain: None,
            ..full.clone()
        };
        assert_eq!(
            scope_invariant_content_digest(&full),
            scope_invariant_content_digest(&stripped),
        );
    }

    #[test]
    fn the_invariant_digest_still_binds_the_summary() {
        // Invariant under stripping must not mean invariant under tampering.
        let a = bundle_with(None, &[]);
        let mut b = a.clone();
        b.summary = "a different summary".into();
        assert_ne!(
            scope_invariant_content_digest(&a),
            scope_invariant_content_digest(&b),
        );
    }

    #[test]
    fn the_reasoning_commitment_catches_an_added_chain_and_allows_a_removed_one() {
        let origin = bundle_with(Some("the real chain"), &[]);
        let commitment = reasoning_commitment(origin.reasoning_chain.as_deref());

        // Removed: the receiver has nothing to compare, which is correct — the
        // chain was withheld by policy.
        let stripped: Option<&str> = None;
        assert_ne!(reasoning_commitment(stripped), commitment);

        // Added by a relayer: detectable.
        assert_ne!(reasoning_commitment(Some("a chain I invented")), commitment);

        // Unchanged: verifies.
        assert_eq!(reasoning_commitment(Some("the real chain")), commitment);
    }

    #[test]
    fn none_and_empty_reasoning_commit_to_the_same_value() {
        assert_eq!(reasoning_commitment(None), reasoning_commitment(Some("")));
    }

    #[test]
    fn a_hop_payload_cannot_be_forged_by_moving_a_field_boundary() {
        // The failure mode `rtx_relay_signing_payload`'s `join("|")` still has:
        // two different field splits producing one identical payload.
        let a = relay_hop_payload("b|c", "o", "sig", 0, "h", "n", "ch", "");
        let b = relay_hop_payload("b", "c|o", "sig", 0, "h", "n", "ch", "");
        assert_ne!(a, b);
    }

    #[test]
    fn a_hop_payload_binds_its_index_and_its_next_peer() {
        let base = relay_hop_payload("b", "o", "sig", 1, "h", "n", "ch", "prev");
        assert_ne!(
            base,
            relay_hop_payload("b", "o", "sig", 2, "h", "n", "ch", "prev"),
            "the hop index must be signed, or a hop can be replayed at another depth",
        );
        assert_ne!(
            base,
            relay_hop_payload("b", "o", "sig", 1, "h", "other", "ch", "prev"),
            "the destination must be signed, or a hop authorises forwarding anywhere",
        );
        assert_ne!(
            base,
            relay_hop_payload("b", "o", "sig", 1, "h", "n", "ch", "other-prev"),
            "the previous hop's digest must be signed, or the chain is a bag",
        );
    }

    fn provenance(hops: &[&str]) -> BundleProvenance {
        BundleProvenance {
            origin_server: "https://a.example".into(),
            relay_path: hops.iter().map(|h| h.to_string()).collect(),
            bundle_id: "b-hop".into(),
            hops: hops
                .iter()
                .map(|h| RelayHop {
                    server: h.to_string(),
                    content_hash: "00".repeat(32),
                    signature: "aa".repeat(64),
                })
                .collect(),
            origin: None,
        }
    }

    #[test]
    fn structure_rejects_a_chain_over_the_ceiling() {
        let too_long: Vec<String> = (0..=RTX_HOP_CEILING)
            .map(|i| format!("https://h{i}.example"))
            .collect();
        let refs: Vec<&str> = too_long.iter().map(|s| s.as_str()).collect();
        let err = validate_provenance_structure(&provenance(&refs)).expect_err("must refuse");
        assert!(format!("{err}").contains("ceiling"), "{err}");
    }

    #[test]
    fn structure_rejects_a_repeated_server() {
        let err = validate_provenance_structure(&provenance(&[
            "https://b.example",
            "https://c.example",
            "https://b.example",
        ]))
        .expect_err("must refuse");
        assert!(format!("{err}").contains("twice"), "{err}");
    }

    #[test]
    fn structure_accepts_an_empty_chain() {
        // A bundle published locally and not yet relayed.
        assert!(validate_provenance_structure(&provenance(&[])).is_ok());
    }

    #[test]
    fn a_chain_is_a_chain_every_payload_depends_on_the_one_before_it() {
        let p = provenance(&["https://b.example", "https://c.example"]);
        let payloads = hop_payloads(&p, "origin-sig", "https://d.example");
        assert_eq!(payloads.len(), 2);

        // Hop 0's next_peer is hop 1's server; hop 1's is the holder.
        assert!(payloads[0].contains("https://c.example"));
        assert!(payloads[1].contains("https://d.example"));

        // Change hop 0's content hash and hop 1's payload moves too, which is
        // what stops a relayer editing an earlier hop and keeping the later
        // signatures.
        let mut edited = p.clone();
        edited.hops[0].content_hash = "ff".repeat(32);
        let after = hop_payloads(&edited, "origin-sig", "https://d.example");
        assert_ne!(payloads[0], after[0]);
        assert_ne!(
            payloads[1], after[1],
            "hop 1's payload must depend on hop 0's, or the chain is a bag of \
             independent claims",
        );
    }

    #[test]
    fn dropping_a_middle_hop_changes_every_payload_after_it() {
        let full = provenance(&[
            "https://b.example",
            "https://c.example",
            "https://d.example",
        ]);
        let spliced = provenance(&["https://b.example", "https://d.example"]);
        let a = hop_payloads(&full, "s", "https://e.example");
        let b = hop_payloads(&spliced, "s", "https://e.example");
        assert_ne!(a[0], b[0], "hop 0's next_peer changed when c was removed");
        assert_ne!(a[2], b[1]);
    }

    #[test]
    fn the_chain_is_bound_to_one_origin_attestation() {
        let p = provenance(&["https://b.example"]);
        assert_ne!(
            hop_payloads(&p, "sig-a", "https://c.example"),
            hop_payloads(&p, "sig-b", "https://c.example"),
            "a hop chain lifted onto a different origin attestation must not verify",
        );
    }

    #[test]
    fn structure_rejects_a_chain_with_no_origin() {
        let mut p = provenance(&["https://b.example"]);
        p.origin_server = String::new();
        assert!(validate_provenance_structure(&p).is_err());
    }
}

#[cfg(test)]
mod redaction_tests {
    use super::*;

    fn bundle(
        domain_tags: &[&str],
        summary: &str,
        reasoning: Option<&str>,
        caveats: &[&str],
    ) -> ReflectionSummaryBundle {
        ReflectionSummaryBundle {
            bundle_id: "b1".into(),
            source_pseudonym: "p1".into(),
            source_server: "http://localhost".into(),
            domain_tags: domain_tags.iter().map(|s| s.to_string()).collect(),
            summary: summary.into(),
            reasoning_chain: reasoning.map(|s| s.to_string()),
            caveats: caveats.iter().map(|s| s.to_string()).collect(),
            created_at: 1,
            signature: "00".into(),
            vrp_handshake_ref: "r".into(),
        }
    }

    #[test]
    fn empty_redaction_list_allows_anything() {
        let b = bundle(&["politics"], "anything about politics", None, &[]);
        assert!(check_redacted_topics(&b, &[]).is_ok());
    }

    #[test]
    fn redacted_domain_tag_is_blocked() {
        let b = bundle(&["politics", "ethics"], "neutral summary", None, &[]);
        assert!(check_redacted_topics(&b, &["politics".into()]).is_err());
    }

    #[test]
    fn redacted_topic_laundered_into_summary_is_blocked() {
        // The tag set is benign, but the prohibited topic appears in the prose.
        let b = bundle(
            &["general"],
            "A deep dive into finance and markets.",
            None,
            &[],
        );
        assert!(
            check_redacted_topics(&b, &["finance".into()]).is_err(),
            "a redacted topic in the summary must be blocked even with a benign tag"
        );
    }

    #[test]
    fn redacted_topic_in_reasoning_or_caveats_is_blocked() {
        let b = bundle(
            &["general"],
            "neutral",
            Some("step 1: discuss politics"),
            &[],
        );
        assert!(check_redacted_topics(&b, &["politics".into()]).is_err());
        let b2 = bundle(&["general"], "neutral", None, &["may touch on Politics"]);
        assert!(
            check_redacted_topics(&b2, &["politics".into()]).is_err(),
            "match is case-insensitive"
        );
    }

    #[test]
    fn substring_of_a_larger_word_is_not_a_false_positive() {
        // "finance" must not match "refinanced".
        let b = bundle(&["general"], "They refinanced the mortgage.", None, &[]);
        assert!(
            check_redacted_topics(&b, &["finance".into()]).is_ok(),
            "whole-word matching must not flag 'refinanced' for redacted 'finance'"
        );
    }

    #[test]
    fn author_signing_payload_binds_every_content_field() {
        // The author payload must change if ANY content field changes — that is
        // what makes the per-agent author signature resist content tampering.
        let base = bundle(
            &["rust", "security"],
            "a summary",
            Some("the reasoning"),
            &["caveat one"],
        );
        let baseline = author_signing_payload(&base);

        let mutate = |f: &dyn Fn(&mut ReflectionSummaryBundle)| {
            let mut b = base.clone();
            f(&mut b);
            author_signing_payload(&b)
        };

        assert_ne!(baseline, mutate(&|b| b.summary = "different".into()));
        assert_ne!(
            baseline,
            mutate(&|b| b.reasoning_chain = Some("changed".into()))
        );
        assert_ne!(baseline, mutate(&|b| b.reasoning_chain = None));
        assert_ne!(baseline, mutate(&|b| b.domain_tags.push("extra".into())));
        assert_ne!(baseline, mutate(&|b| b.caveats.push("extra".into())));
        assert_ne!(baseline, mutate(&|b| b.bundle_id = "other".into()));
        assert_ne!(baseline, mutate(&|b| b.source_pseudonym = "other".into()));
        assert_ne!(baseline, mutate(&|b| b.source_server = "other".into()));
        assert_ne!(baseline, mutate(&|b| b.vrp_handshake_ref = "other".into()));
        assert_ne!(baseline, mutate(&|b| b.created_at = 999));

        // The `signature` field itself is deliberately NOT bound (it's what gets
        // signed), so changing it does not change the payload.
        assert_eq!(baseline, mutate(&|b| b.signature = "deadbeef".into()));

        // Deterministic.
        assert_eq!(baseline, author_signing_payload(&base));
    }

    #[test]
    fn author_signing_payload_resists_field_boundary_confusion() {
        // Length-prefixing must prevent "ab"+"c" colliding with "a"+"bc".
        let mut x = bundle(&[], "s", None, &[]);
        x.source_pseudonym = "ab".into();
        x.source_server = "c".into();
        let mut y = bundle(&[], "s", None, &[]);
        y.source_pseudonym = "a".into();
        y.source_server = "bc".into();
        assert_ne!(author_signing_payload(&x), author_signing_payload(&y));
    }
}
