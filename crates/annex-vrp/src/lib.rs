//! VRP (Value Resonance Protocol) trust negotiation for the Annex platform.
//!
//! Implements the trust negotiation layer: anchor comparison (`compare_peer_anchor`),
//! transfer scope negotiation, capability contract evaluation, and reputation
//! tracking. Adapted from the MABOS `value_resonance` module for the Annex
//! server-agent and server-server contexts.
//!
//! VRP is the mechanism by which Annex enforces cryptographic trust rather than
//! administrative trust. Every agent connection and every federation agreement
//! is mediated by a VRP handshake that compares ethical/policy roots, evaluates
//! capability contracts, checks longitudinal reputation, and produces an
//! alignment classification (`Aligned`, `Partial`, or `Conflict`).
//!
//! # Phase 3 implementation
//!
//! The full implementation of this crate is Phase 3 of the roadmap. The
//! current skeleton provides the module structure that will be filled in
//! during that phase.

pub mod reputation;
pub mod semantic;
pub mod server_root;
pub mod types;

#[cfg(test)]
mod tests;

pub use reputation::{check_reputation_score, record_vrp_outcome, ReputationError};
pub use server_root::ServerPolicyRoot;
pub use types::{
    VrpAlignmentConfig, VrpAlignmentStatus, VrpAnchorSnapshot, VrpCapabilitySharingContract,
    VrpFederationHandshake, VrpTransferAcceptanceConfig, VrpTransferAcceptanceError,
    VrpTransferScope, VrpValidationReport,
};

use sha2::{Digest, Sha256};
use std::collections::HashSet;

/// Creates a SHA256 hash of a list of strings, sorted to ensure determinism.
fn hash_list(items: &[String]) -> String {
    let mut sorted_items = items.to_vec();
    sorted_items.sort();
    let mut hasher = Sha256::new();
    for item in sorted_items {
        // Length prefix to prevent collisions (e.g. "ab", "c" vs "a", "bc")
        hasher.update((item.len() as u64).to_be_bytes());
        hasher.update(item.as_bytes());
    }
    hex::encode(hasher.finalize())
}

impl VrpAnchorSnapshot {
    /// Creates a new snapshot from principles and prohibited actions.
    pub fn new(principles: &[String], prohibited_actions: &[String]) -> Self {
        Self {
            principles_hash: hash_list(principles),
            prohibited_actions_hash: hash_list(prohibited_actions),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        }
    }
}

/// Compares two anchor snapshots to determine alignment status.
///
/// In Phase 3, this performs an exact hash match.
///
/// Note: Semantic alignment (embedding comparison) for partial matches is defined in the
/// `semantic` module. However, because `VrpAnchorSnapshot` only contains hashes of the
/// principles, this function currently only supports exact matches (Aligned) or
/// mismatches (Conflict). To support partial alignment, the full principle text
/// must be available and passed to `semantic::calculate_semantic_alignment`.
pub fn compare_peer_anchor(
    local: &VrpAnchorSnapshot,
    remote: &VrpAnchorSnapshot,
    _config: &VrpAlignmentConfig,
) -> VrpAlignmentStatus {
    if local.principles_hash == remote.principles_hash
        && local.prohibited_actions_hash == remote.prohibited_actions_hash
    {
        VrpAlignmentStatus::Aligned
    } else {
        // Without semantic analysis, any difference is treated as a conflict for safety.
        // If semantic alignment was implemented, we would compute a score here.
        VrpAlignmentStatus::Conflict
    }
}

/// Validates that capability contracts are mutually compatible.
///
/// Returns true if:
/// 1. Local offered capabilities cover all remote required capabilities.
/// 2. Remote offered capabilities cover all local required capabilities.
pub fn contracts_mutually_accepted(
    local: &VrpCapabilitySharingContract,
    remote: &VrpCapabilitySharingContract,
) -> bool {
    let local_offered: HashSet<String> = local.offered_capabilities.iter().cloned().collect();
    let remote_offered: HashSet<String> = remote.offered_capabilities.iter().cloned().collect();

    let remote_required_satisfied = remote
        .required_capabilities
        .iter()
        .all(|req| local_offered.contains(req));

    let local_required_satisfied = local
        .required_capabilities
        .iter()
        .all(|req| remote_offered.contains(req));

    remote_required_satisfied && local_required_satisfied
}

/// Resolves the transfer scope based on alignment status and local acceptance config.
pub fn resolve_transfer_scope(
    status: VrpAlignmentStatus,
    config: &VrpTransferAcceptanceConfig,
) -> VrpTransferScope {
    match status {
        VrpAlignmentStatus::Aligned => {
            if config.allow_full_knowledge {
                VrpTransferScope::FullKnowledgeBundle
            } else if config.allow_reflection_summaries {
                VrpTransferScope::ReflectionSummariesOnly
            } else {
                VrpTransferScope::NoTransfer
            }
        }
        VrpAlignmentStatus::Partial => {
            if config.allow_reflection_summaries {
                VrpTransferScope::ReflectionSummariesOnly
            } else {
                VrpTransferScope::NoTransfer
            }
        }
        VrpAlignmentStatus::Conflict => VrpTransferScope::NoTransfer,
    }
}

/// Validates a full federation handshake against local policy and state.
pub fn validate_federation_handshake(
    local_anchor: &VrpAnchorSnapshot,
    local_contract: &VrpCapabilitySharingContract,
    handshake: &VrpFederationHandshake,
    alignment_config: &VrpAlignmentConfig,
    transfer_config: &VrpTransferAcceptanceConfig,
) -> VrpValidationReport {
    // 1. Compare anchors
    let alignment_status =
        compare_peer_anchor(local_anchor, &handshake.anchor_snapshot, alignment_config);

    // 2. Check capability contracts
    let contracts_ok = contracts_mutually_accepted(local_contract, &handshake.capability_contract);

    let mut notes = Vec::new();
    let final_status = if !contracts_ok {
        notes.push("Capability contracts incompatible".to_string());
        // Downgrade status if contracts fail.
        // Even if Aligned on principles, incompatible capabilities mean we can't fully interoperate.
        // We treat this as a conflict for now to prevent broken connections.
        VrpAlignmentStatus::Conflict
    } else {
        alignment_status
    };

    // 3. Resolve transfer scope
    let transfer_scope = resolve_transfer_scope(final_status, transfer_config);

    // Score is 1.0 for Aligned, 0.0 for Conflict (placeholder for now)
    let alignment_score = match final_status {
        VrpAlignmentStatus::Aligned => 1.0,
        VrpAlignmentStatus::Partial => 0.5,
        VrpAlignmentStatus::Conflict => 0.0,
    };

    VrpValidationReport {
        alignment_status: final_status,
        transfer_scope,
        alignment_score,
        negotiation_notes: notes,
    }
}

/// Validates whether a validation report meets the requirements for a specific transfer scope.
///
/// This function is used to gate data transfers (e.g., RTX bundles) based on the
/// negotiated VRP alignment and transfer scope.
pub fn check_transfer_acceptance(
    report: &VrpValidationReport,
    required_scope: VrpTransferScope,
) -> Result<(), VrpTransferAcceptanceError> {
    if report.alignment_status == VrpAlignmentStatus::Conflict {
        return Err(VrpTransferAcceptanceError::Conflict);
    }

    if report.transfer_scope < required_scope {
        return Err(VrpTransferAcceptanceError::Rejected(format!(
            "Insufficient transfer scope: negotiated {}, required {}",
            report.transfer_scope, required_scope
        )));
    }

    Ok(())
}
