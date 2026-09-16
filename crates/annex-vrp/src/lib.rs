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

pub mod embedding;
pub mod reputation;
pub mod scorer;
pub mod semantic;
pub mod server_root;
pub mod types;

#[cfg(test)]
mod tests;

pub use reputation::{check_reputation_score, record_vrp_outcome, ReputationError};
pub use server_root::ServerPolicyRoot;
pub use types::{
    ScoringProvenance, VrpAlignmentConfig, VrpAlignmentStatus, VrpAnchorSnapshot,
    VrpCapabilitySharingContract, VrpError, VrpFederationHandshake, VrpTransferAcceptanceConfig,
    VrpTransferAcceptanceError, VrpTransferScope, VrpValidationReport,
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

/// The score reported when the scorer itself failed.
///
/// Every real score is a cosine in `[0, 1]`, so a negative value cannot be
/// confused with a measurement. `0.0` could: it is what a genuinely
/// unrelated pair scores, and what the "no comparison applies" branches
/// return.
pub const UNMEASURABLE_SCORE: f32 = -1.0;

/// Like [`compare_peer_anchor`] but also returns the *measured* anchor
/// similarity (0.0–1.0), so callers can record the real number rather than a
/// status-derived placeholder. The score is `1.0` on an exact match, `0.0` on a
/// prohibited-action divergence (or when no semantic comparison is possible),
/// and the measured cosine in the semantic branch — independent of whether that
/// value cleared `min_alignment_score`. [`UNMEASURABLE_SCORE`] means the scorer
/// itself failed and nothing was measured; it is negative precisely so it
/// cannot be mistaken for one of the above.
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

    // A server that has declared no principles has stated no requirement.
    //
    // `ServerPolicy::default()` ships `principles: []` alongside
    // `agent_min_alignment_score: 0.8`, and the handshake hardcodes
    // `semantic_alignment_required: true`. With an empty local list the
    // semantic branch below was unreachable, so control fell to the
    // `Conflict` at the end of this function and every agent that declared
    // an ethical anchor was rejected — on a stock server, the only agent
    // that could ever be admitted was one whose principles AND prohibited
    // actions were both empty, matching the empty local anchor by hash.
    // The threshold was never consulted, and nothing told the operator why
    // agent registration always failed.
    //
    // Rejecting on a comparison that cannot be made is not fail-closed, it
    // is arbitrary: there is no declared value for the agent to conflict
    // with. Prohibited actions are different and are still enforced above —
    // those are a boundary the operator actually stated.
    if local.principles.is_empty() {
        return (VrpAlignmentStatus::Aligned, 1.0);
    }

    // Semantic alignment: compare original principle text when available.
    // Only reachable when prohibited actions already match (above).
    if config.semantic_alignment_required
        && !local.principles.is_empty()
        && !remote.principles.is_empty()
    {
        // The scorer this process installed — `StaticEmbedder` over the pinned
        // potion-base-2M table on a server, the lexicon otherwise. This used to
        // construct `ConceptEmbedder::new()` inline, which meant the scorer was
        // not a deployment choice and the real model was reachable from
        // nothing. See `crate::scorer`.
        let embedder = scorer::active();

        let measured = semantic::calculate_semantic_alignment(
            &local.principles,
            &remote.principles,
            embedder.as_ref(),
        );

        // A scorer that could not produce a number is a fault in this server,
        // not evidence about the peer, and it used to be neither reported nor
        // distinguishable: the `if let Ok(..)` fell through to the
        // `(Conflict, 0.0)` at the end of this function, which is also what a
        // genuine measured zero returns and also what "no semantic comparison
        // applies" returns. Three different situations, one answer, no log
        // line. `StaticEmbedder::embed` can fail on a token id outside its
        // table, so this is reachable.
        //
        // The verdict stays `Conflict` — refusing is the right default for a
        // trust decision this server cannot evaluate — but it says so, and the
        // score is -1.0 rather than 0.0 so an operator reading a stored row or
        // a handshake report can tell "we could not measure" from "we measured
        // nothing in common". A score is otherwise a cosine in [0, 1], so a
        // negative value is unambiguous.
        let Ok(raw) = measured else {
            if let Err(e) = measured {
                tracing::warn!(
                    error = %e,
                    scorer = %embedder.fingerprint().model_id,
                    "alignment could not be measured; refusing rather than guessing",
                );
            }
            return (VrpAlignmentStatus::Conflict, UNMEASURABLE_SCORE);
        };

        {
            // Three steps, and each one is load-bearing:
            //
            //  1. Normalise against THIS scorer's measured noise floor, so the
            //     configured threshold means the same strictness whichever
            //     scorer is loaded. The raw cosines are not portable: the
            //     static model's unrelated pairs top out at 0.5134 and the
            //     lexicon's at 0.3060, and their separating bands do not
            //     overlap, so one raw number cannot serve both.
            //  2. Quantise, so two peers on the same model but different
            //     hardware cannot land on opposite sides of the threshold over
            //     a last-bit difference in float accumulation.
            //  3. Compare — and return the RAW score, because that is the
            //     measurement an operator should see. `0.5740 under
            //     potion-base-2M` is a fact about the world; the normalised
            //     value is a fact about this scale.
            let normalized = embedding::quantize_score(semantic::normalize_against_floor(
                raw,
                embedder.unrelated_floor(),
            ));
            if normalized >= config.min_alignment_score {
                return (VrpAlignmentStatus::Partial, raw);
            }
            return (VrpAlignmentStatus::Conflict, raw);
        }
    }

    (VrpAlignmentStatus::Conflict, 0.0)
}

/// Everything [`compare_peer_anchor_scored`] decided, and what it decided it
/// with.
///
/// The two-value return above cannot say which scorer produced the number, and
/// a peer's verdict is only reproducible by someone running the same one. An
/// operator looking at a `Conflict` needs to be able to tell "we disagree about
/// values" from "we are measuring with different rulers".
#[derive(Debug, Clone, PartialEq)]
pub struct AlignmentOutcome {
    pub status: VrpAlignmentStatus,
    /// Cosine similarity between the principle-set centroids, as measured.
    pub raw_score: f32,
    /// [`semantic::normalize_against_floor`] applied to `raw_score`, quantised.
    /// This is the value that met (or missed) `min_alignment_score`.
    pub normalized_score: f32,
    /// The threshold it was compared against.
    pub threshold: f32,
    /// The scorer that produced `raw_score`.
    pub scorer: embedding::ModelFingerprint,
}

/// [`compare_peer_anchor_scored`] with the workings shown.
pub fn compare_peer_anchor_detailed(
    local: &VrpAnchorSnapshot,
    remote: &VrpAnchorSnapshot,
    config: &VrpAlignmentConfig,
) -> AlignmentOutcome {
    let (status, raw_score) = compare_peer_anchor_scored(local, remote, config);
    let embedder = scorer::active();
    AlignmentOutcome {
        status,
        raw_score,
        normalized_score: embedding::quantize_score(semantic::normalize_against_floor(
            raw_score,
            embedder.unrelated_floor(),
        )),
        threshold: config.min_alignment_score,
        scorer: embedder.fingerprint(),
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
    // 1. Compare anchors — keep the *measured* similarity, not just the status.
    let (alignment_status, alignment_score) =
        compare_peer_anchor_scored(local_anchor, &handshake.anchor_snapshot, alignment_config);

    // 2. Check capability contracts
    let contracts_ok = contracts_mutually_accepted(local_contract, &handshake.capability_contract);

    let mut notes = Vec::new();

    // 1b. Record what each side measured with.
    //
    // A mismatch does NOT change the local verdict — this server embedded both
    // principle sets with its own scorer, so its number is internally sound.
    // What a mismatch means is that the peer will very likely reach a different
    // verdict about us, and an operator looking at a federation that works in
    // one direction needs to be able to see why. It is a note and a recorded
    // fact, not a refusal: refusing would cut off every peer that has not yet
    // installed the same model, which is a worse outcome than an asymmetry the
    // operator can see.
    let scoring = ScoringProvenance::new(scorer::active_fingerprint(), handshake.scorer.clone());
    if scoring.mismatched {
        notes.push(format!(
            "peer scores alignment with '{}', this server with '{}': the peer's own \
             verdict about us will not match ours about it",
            scoring
                .remote
                .as_ref()
                .map(|r| r.model_id.as_str())
                .unwrap_or("unknown"),
            scoring.local.model_id,
        ));
    }

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
        scoring: Some(scoring),
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
