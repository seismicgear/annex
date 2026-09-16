//! A scorer that cannot produce a number must not look like a measurement.
//!
//! `compare_peer_anchor_scored` used to run its semantic branch as
//! `if let Ok(raw) = calculate_semantic_alignment(..) { .. }` with no else, so
//! an embedder error fell through to the `(Conflict, 0.0)` at the end of the
//! function. That is the same value returned for a genuinely unrelated pair and
//! the same value returned when no semantic comparison applies at all: three
//! distinct situations collapsed into one answer, with no log line and nothing
//! stored to tell them apart. `StaticEmbedder::embed` can fail on a token id
//! outside its table, so the branch is reachable rather than theoretical.
//!
//! This lives in its own integration test because `scorer::install` writes a
//! process-global `OnceLock`: a failing scorer installed in a shared binary
//! would decide every other test in it.

use annex_vrp::embedding::ModelFingerprint;
use annex_vrp::semantic::SemanticEmbedder;
use annex_vrp::{
    compare_peer_anchor_detailed, compare_peer_anchor_scored, VrpAlignmentConfig,
    VrpAlignmentStatus, VrpAnchorSnapshot, UNMEASURABLE_SCORE,
};
use std::sync::Arc;

/// An embedder that fails the way a real one can: some inputs work, and one
/// does not.
struct BrokenEmbedder;

impl SemanticEmbedder for BrokenEmbedder {
    fn embed(&self, _text: &str) -> Result<Vec<f32>, String> {
        Err("token id 30000 is outside the embedding table (29528 rows)".to_string())
    }

    fn unrelated_floor(&self) -> f32 {
        0.5
    }

    fn fingerprint(&self) -> ModelFingerprint {
        ModelFingerprint {
            model_id: "broken-for-test".to_string(),
            weights_sha256: "0".repeat(64),
        }
    }
}

#[test]
fn a_scorer_that_fails_is_not_reported_as_a_measurement_of_zero() {
    annex_vrp::scorer::install(Arc::new(BrokenEmbedder)).expect("first install in this process");

    // Same prohibited actions (so the divergence branch above does not fire),
    // different principles (so the hash fast-path does not), and both sides
    // non-empty (so the semantic branch is the one that runs).
    let local = VrpAnchorSnapshot::new(
        &["treat every participant as a peer".to_string()],
        &["impersonation".to_string()],
    )
    .unwrap();
    let remote = VrpAnchorSnapshot::new(
        &["never speak for someone else".to_string()],
        &["impersonation".to_string()],
    )
    .unwrap();

    let config = VrpAlignmentConfig {
        semantic_alignment_required: true,
        min_alignment_score: 0.06,
    };

    let (status, score) = compare_peer_anchor_scored(&local, &remote, &config);

    // Refusing is right: this server cannot evaluate the trust question, and
    // guessing either way would be worse. What matters is that it is legible.
    assert_eq!(status, VrpAlignmentStatus::Conflict);
    assert_eq!(
        score, UNMEASURABLE_SCORE,
        "a scoring fault must be distinguishable from a measured 0.0",
    );
    assert!(
        score < 0.0,
        "every real score is a cosine in [0, 1], so the sentinel has to be outside that",
    );

    // And the detailed report carries the failing scorer's identity, so an
    // operator reading a stored Conflict can tell which instrument produced it.
    let detailed = compare_peer_anchor_detailed(&local, &remote, &config);
    assert_eq!(detailed.status, VrpAlignmentStatus::Conflict);
    assert_eq!(detailed.raw_score, UNMEASURABLE_SCORE);
    assert_eq!(detailed.scorer.model_id, "broken-for-test");
    assert!(
        detailed.normalized_score >= 0.0,
        "the normalised value is clamped into [0, 1] by construction; only the raw \
         score carries the sentinel",
    );
}

#[test]
fn a_pair_that_needs_no_measurement_is_unaffected_by_a_broken_scorer() {
    // The hash fast-path and the prohibited-action divergence must not start
    // reporting UNMEASURABLE_SCORE just because the scorer is broken — they
    // never consult it.
    let a = VrpAnchorSnapshot::new(&["be kind".to_string()], &["spam".to_string()]).unwrap();
    let config = VrpAlignmentConfig {
        semantic_alignment_required: true,
        min_alignment_score: 0.06,
    };
    assert_eq!(
        compare_peer_anchor_scored(&a, &a, &config),
        (VrpAlignmentStatus::Aligned, 1.0),
    );

    let diverging =
        VrpAnchorSnapshot::new(&["be kind".to_string()], &["nothing".to_string()]).unwrap();
    assert_eq!(
        compare_peer_anchor_scored(&a, &diverging, &config),
        (VrpAlignmentStatus::Conflict, 0.0),
        "a stated-boundary divergence is a measurement, not a fault",
    );
}
