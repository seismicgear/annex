//! Validation and transfer scope enforcement for RTX bundles.
//!
//! This module provides the logic for enforcing VRP transfer scope on
//! reflection summary bundles, checking for redacted topics, and
//! validating bundle structure before publish or delivery.

use crate::error::RtxError;
use crate::types::ReflectionSummaryBundle;
use annex_vrp::VrpTransferScope;

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

/// Validates that a bundle has all required fields populated.
///
/// This performs structural validation only — it does not verify
/// the cryptographic signature (that requires the sender's public key).
pub fn validate_bundle_structure(bundle: &ReflectionSummaryBundle) -> Result<(), RtxError> {
    if bundle.bundle_id.is_empty() {
        return Err(RtxError::InvalidBundle("bundle_id is empty".to_string()));
    }
    if bundle.source_pseudonym.is_empty() {
        return Err(RtxError::InvalidBundle(
            "source_pseudonym is empty".to_string(),
        ));
    }
    if bundle.source_server.is_empty() {
        return Err(RtxError::InvalidBundle(
            "source_server is empty".to_string(),
        ));
    }
    if bundle.summary.is_empty() {
        return Err(RtxError::InvalidBundle("summary is empty".to_string()));
    }
    if bundle.signature.is_empty() {
        return Err(RtxError::InvalidBundle("signature is empty".to_string()));
    }
    if bundle.vrp_handshake_ref.is_empty() {
        return Err(RtxError::InvalidBundle(
            "vrp_handshake_ref is empty".to_string(),
        ));
    }
    if bundle.created_at == 0 {
        return Err(RtxError::InvalidBundle(
            "created_at must be non-zero".to_string(),
        ));
    }
    Ok(())
}

/// Constructs the signing payload for a bundle.
///
/// The signed message is: `bundle_id + source_pseudonym + source_server + summary + created_at`.
/// Callers should SHA256-hash this payload and sign the hash with Ed25519.
pub fn bundle_signing_payload(bundle: &ReflectionSummaryBundle) -> String {
    format!(
        "{}{}{}{}{}",
        bundle.bundle_id,
        bundle.source_pseudonym,
        bundle.source_server,
        bundle.summary,
        bundle.created_at
    )
}
