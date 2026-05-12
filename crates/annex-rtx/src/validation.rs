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

/// Checks whether a bundle contains any redacted topics.
///
/// Returns an error if any of the bundle's `domain_tags` appear in the
/// redacted topics list from the sender's capability contract. Redacted
/// topics represent knowledge domains that the agent is prohibited from
/// sharing per its VRP agreement.
pub fn check_redacted_topics(
    bundle: &ReflectionSummaryBundle,
    redacted_topics: &[String],
) -> Result<(), RtxError> {
    for tag in &bundle.domain_tags {
        if redacted_topics.contains(tag) {
            return Err(RtxError::RedactedTopic(tag.clone()));
        }
    }
    Ok(())
}

/// Validates that a bundle has all required fields populated AND that
/// every variable-length field is within sane size bounds.
///
/// This performs structural validation only — it does not verify
/// the cryptographic signature (that requires the sender's public key).
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
