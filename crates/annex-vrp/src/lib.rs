//! VRP (Value Resonance Protocol) trust negotiation for the Annex platform.
//!
//! Implements the trust negotiation layer: anchor comparison (`compare_peer_anchor`),
//! transfer scope negotiation, capability contract evaluation, and reputation
//! tracking. Adapted from the MABOS `value_resonance` module for the Annex
//! server-agent and server-server contexts.
//!
//! VRP is the mechanism by which Annex mediates agent and federation trust.
//! Every agent connection and every federation agreement is mediated by a VRP
//! handshake that compares ethical/policy roots and evaluates capability
//! contracts to produce an alignment classification (`Aligned`, `Partial`, or
//! `Conflict`).
//!
//! NOTE on reputation: the base verdict from [`validate_federation_handshake`]
//! is gated by longitudinal reputation via [`apply_reputation_gate`] — a peer
//! whose history of `Partial`/`Conflict` outcomes has driven its reputation
//! below [`MIN_REPUTATION_FOR_FULL_ALIGNMENT`] is downgraded one alignment step.
//! Callers (see `api_vrp`) read the reputation score from prior history before
//! recording the current outcome, then apply the gate.
//!
//! NOTE on "semantic" alignment: the default embedder is
//! [`semantic::ConceptEmbedder`] — a fixed-dimension, paraphrase-aware concept
//! embedding (synonym families share a concept dimension, plus char-trigram
//! hashing for morphology). It is deterministic and dependency-free, so two
//! federated peers embed principles into the same space with no shared
//! vocabulary, and paraphrased-but-aligned principles are no longer reflexively
//! `Conflict`. It is honestly NOT a learned neural model; the
//! [`semantic::SemanticEmbedder`] trait keeps one pluggable for deployments
//! that accept the size/latency cost (ROADMAP 3.3). The legacy
//! [`semantic::BagOfWordsEmbedder`] is retained for comparison/tests.
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
    VrpError, VrpFederationHandshake, VrpTransferAcceptanceConfig, VrpTransferAcceptanceError,
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
    ///
    /// Returns `VrpError::SystemClockInvalid` if the system clock is before
    /// the UNIX epoch, which would produce an invalid timestamp.
    pub fn new(principles: &[String], prohibited_actions: &[String]) -> Result<Self, VrpError> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| VrpError::SystemClockInvalid)?
            .as_secs();

        Ok(Self {
            principles_hash: hash_list(principles),
            prohibited_actions_hash: hash_list(prohibited_actions),
            timestamp,
            principles: principles.to_vec(),
            prohibited_actions: prohibited_actions.to_vec(),
        })
    }
}

/// Compares two anchor snapshots to determine alignment status.
///
/// 1. Exact hash match on both principles and prohibited actions → `Aligned`
/// 2. If hashes differ and original text is available on both sides, computes
///    bag-of-words semantic similarity. Score >= `config.min_alignment_score` → `Partial`
/// 3. Otherwise → `Conflict`
pub fn compare_peer_anchor(
    local: &VrpAnchorSnapshot,
    remote: &VrpAnchorSnapshot,
    config: &VrpAlignmentConfig,
) -> VrpAlignmentStatus {
    compare_peer_anchor_scored(local, remote, config).0
}

/// Like [`compare_peer_anchor`] but also returns the *measured* anchor
/// similarity (0.0–1.0), so callers can record the real number rather than a
/// status-derived placeholder. The score is `1.0` on an exact match, `0.0` on a
/// prohibited-action divergence (or when no semantic comparison is possible),
/// and the bag-of-words cosine value in the semantic branch — independent of
/// whether that value cleared `min_alignment_score`.
pub fn compare_peer_anchor_scored(
    local: &VrpAnchorSnapshot,
    remote: &VrpAnchorSnapshot,
    config: &VrpAlignmentConfig,
) -> (VrpAlignmentStatus, f32) {
    // Fast path: exact hash match
    if local.principles_hash == remote.principles_hash
        && local.prohibited_actions_hash == remote.prohibited_actions_hash
    {
        return (VrpAlignmentStatus::Aligned, 1.0);
    }

    // Prohibited-action divergence is an immediate conflict regardless of
    // principle similarity. Allowing Partial when prohibitions differ would
    // let peers with conflicting safety boundaries negotiate transfer scopes
    // they shouldn't have.
    if local.prohibited_actions_hash != remote.prohibited_actions_hash {
        return (VrpAlignmentStatus::Conflict, 0.0);
    }

    // Semantic alignment: compare original principle text when available.
    // Only reachable when prohibited actions already match (above).
    if config.semantic_alignment_required
        && !local.principles.is_empty()
        && !remote.principles.is_empty()
    {
        // Concept embedding: fixed-dimension, no jointly-built vocabulary, and
        // paraphrase-aware (synonym families share a concept dimension). A
        // federated peer's principles embed into the SAME space as ours
        // natively, and "users deserve privacy" ≈ "people are entitled to
        // confidentiality" instead of scoring ~0 as bag-of-words did.
        let embedder = semantic::ConceptEmbedder::new();

        if let Ok(score) =
            semantic::calculate_semantic_alignment(&local.principles, &remote.principles, &embedder)
        {
            if score >= config.min_alignment_score {
                return (VrpAlignmentStatus::Partial, score);
            }
            // Below threshold → Conflict, but surface the real measured score.
            return (VrpAlignmentStatus::Conflict, score);
        }
    }

    (VrpAlignmentStatus::Conflict, 0.0)
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
    // 1. Compare anchors — keep the *measured* similarity, not just the status.
    let (alignment_status, alignment_score) =
        compare_peer_anchor_scored(local_anchor, &handshake.anchor_snapshot, alignment_config);

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

    // `alignment_score` is the measured anchor similarity from step 1 — it is
    // NOT recomputed from `final_status`. The status is the verdict (anchors +
    // contracts); the score reports how similar the anchors actually were.
    VrpValidationReport {
        alignment_status: final_status,
        transfer_scope,
        alignment_score,
        negotiation_notes: notes,
    }
}

/// Minimum longitudinal reputation a peer must retain to be admitted at the
/// alignment its anchors/contracts earned this round.
///
/// The reputation score is neutral at 0.5 and only falls below this after a
/// *sustained* history of `Partial`/`Conflict` outcomes — a single bad
/// handshake from a fresh peer stays well above it — so the gate targets
/// repeat offenders, not newcomers.
pub const MIN_REPUTATION_FOR_FULL_ALIGNMENT: f32 = 0.25;

/// Applies the longitudinal-reputation gate to a freshly-computed report.
///
/// When `reputation_score` is healthy (>= [`MIN_REPUTATION_FOR_FULL_ALIGNMENT`])
/// the report is returned unchanged. Otherwise the alignment is downgraded one
/// step — `Aligned` → `Partial`, `Partial` → `Conflict` — and the transfer
/// scope and score are recomputed for the new status.
///
/// This is what makes reputation actually affect the outcome (ROADMAP Phase 3
/// completion criterion): a peer with a poor track record cannot be freely
/// re-admitted as `Aligned` on the strength of a single good anchor comparison.
/// Callers must read `reputation_score` from history *before* recording the
/// current outcome so it reflects past behaviour.
pub fn apply_reputation_gate(
    mut report: VrpValidationReport,
    reputation_score: f32,
    transfer_config: &VrpTransferAcceptanceConfig,
) -> VrpValidationReport {
    if reputation_score >= MIN_REPUTATION_FOR_FULL_ALIGNMENT {
        return report;
    }
    let downgraded = match report.alignment_status {
        VrpAlignmentStatus::Aligned => Some(VrpAlignmentStatus::Partial),
        VrpAlignmentStatus::Partial => Some(VrpAlignmentStatus::Conflict),
        VrpAlignmentStatus::Conflict => None,
    };
    if let Some(new_status) = downgraded {
        report.negotiation_notes.push(format!(
            "alignment downgraded {} -> {} due to low longitudinal reputation ({reputation_score:.2} < {MIN_REPUTATION_FOR_FULL_ALIGNMENT:.2})",
            report.alignment_status, new_status
        ));
        report.alignment_status = new_status;
        report.transfer_scope = resolve_transfer_scope(new_status, transfer_config);
        // `alignment_score` is the measured anchor similarity and is left
        // untouched — only the verdict (status/scope) is downgraded.
    }
    report
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
