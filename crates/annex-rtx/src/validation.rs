//! Validation and transfer scope enforcement for RTX bundles.
//!
//! This module provides the logic for enforcing VRP transfer scope on
//! reflection summary bundles, checking for redacted topics, and
//! validating bundle structure before publish or delivery.

use crate::error::RtxError;
use crate::types::ReflectionSummaryBundle;
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

        let mut mutate = |f: &dyn Fn(&mut ReflectionSummaryBundle)| {
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
