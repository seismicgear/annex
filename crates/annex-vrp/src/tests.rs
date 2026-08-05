use crate::*;

#[test]
fn test_vrp_alignment_status_serialization() {
    let status = VrpAlignmentStatus::Aligned;
    let json = serde_json::to_string(&status).unwrap();
    assert_eq!(json, "\"Aligned\"");
    let deserialized: VrpAlignmentStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, status);
}

#[test]
fn test_vrp_transfer_scope_serialization() {
    let scope = VrpTransferScope::ReflectionSummariesOnly;
    let json = serde_json::to_string(&scope).unwrap();
    assert_eq!(json, "\"ReflectionSummariesOnly\"");
    let deserialized: VrpTransferScope = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, scope);
}

#[test]
fn test_vrp_anchor_snapshot_serialization() {
    let snapshot = VrpAnchorSnapshot {
        principles_hash: "0x123".to_string(),
        prohibited_actions_hash: "0x456".to_string(),
        timestamp: 1234567890,
        principles: vec![],
        prohibited_actions: vec![],
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    let deserialized: VrpAnchorSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, snapshot);
}

#[test]
fn test_vrp_federation_handshake_serialization() {
    let handshake = VrpFederationHandshake {
        anchor_snapshot: VrpAnchorSnapshot {
            principles_hash: "0xabc".to_string(),
            prohibited_actions_hash: "0xdef".to_string(),
            timestamp: 100,
            principles: vec![],
            prohibited_actions: vec![],
        },
        capability_contract: VrpCapabilitySharingContract {
            required_capabilities: vec!["cap1".to_string()],
            offered_capabilities: vec!["cap2".to_string()],
            redacted_topics: vec![],
        },
    };
    let json = serde_json::to_string(&handshake).unwrap();
    let deserialized: VrpFederationHandshake = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, handshake);
}

#[test]
fn test_vrp_validation_report_serialization() {
    let report = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Partial,
        transfer_scope: VrpTransferScope::NoTransfer,
        alignment_score: 0.5,
        negotiation_notes: vec!["note1".to_string()],
    };
    let json = serde_json::to_string(&report).unwrap();
    let deserialized: VrpValidationReport = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized, report);
}

#[test]
fn test_display_impls() {
    assert_eq!(VrpAlignmentStatus::Aligned.to_string(), "ALIGNED");
    assert_eq!(VrpAlignmentStatus::Partial.to_string(), "PARTIAL");
    assert_eq!(VrpAlignmentStatus::Conflict.to_string(), "CONFLICT");

    assert_eq!(VrpTransferScope::NoTransfer.to_string(), "NO_TRANSFER");
    assert_eq!(
        VrpTransferScope::ReflectionSummariesOnly.to_string(),
        "REFLECTION_SUMMARIES_ONLY"
    );
    assert_eq!(
        VrpTransferScope::FullKnowledgeBundle.to_string(),
        "FULL_KNOWLEDGE_BUNDLE"
    );
}

#[test]
fn test_anchor_snapshot_determinism() {
    let p1 = vec!["principle A".to_string(), "principle B".to_string()];
    let p2 = vec!["principle B".to_string(), "principle A".to_string()];
    let prohibited = vec!["bad action".to_string()];

    let snap1 = VrpAnchorSnapshot::new(&p1, &prohibited).unwrap();
    let snap2 = VrpAnchorSnapshot::new(&p2, &prohibited).unwrap();

    assert_eq!(snap1.principles_hash, snap2.principles_hash);
    assert_eq!(snap1.prohibited_actions_hash, snap2.prohibited_actions_hash);
    // Timestamps might differ slightly, so ignore them for hash check
}

#[test]
fn test_compare_peer_anchor_aligned() {
    let principles = vec!["p1".to_string()];
    let prohibited = vec!["no1".to_string()];
    let snap1 = VrpAnchorSnapshot::new(&principles, &prohibited).unwrap();
    // Clone via serialize/deserialize to simulate remote
    let snap2 = snap1.clone();

    let config = VrpAlignmentConfig {
        semantic_alignment_required: false,
        min_alignment_score: 0.8,
    };

    let status = compare_peer_anchor(&snap1, &snap2, &config);
    assert_eq!(status, VrpAlignmentStatus::Aligned);
}

#[test]
fn test_compare_peer_anchor_conflict() {
    let snap1 = VrpAnchorSnapshot::new(&["p1".to_string()], &[]).unwrap();
    let snap2 = VrpAnchorSnapshot::new(&["p2".to_string()], &[]).unwrap(); // different

    let config = VrpAlignmentConfig {
        semantic_alignment_required: false,
        min_alignment_score: 0.8,
    };

    let status = compare_peer_anchor(&snap1, &snap2, &config);
    assert_eq!(status, VrpAlignmentStatus::Conflict);
}

#[test]
fn test_compare_peer_anchor_prohibited_action_mismatch_forces_conflict() {
    // Even with identical principles, differing prohibited actions must yield Conflict.
    let snap1 = VrpAnchorSnapshot::new(
        &["shared principle".to_string()],
        &["no violence".to_string()],
    )
    .unwrap();
    let snap2 = VrpAnchorSnapshot::new(
        &["shared principle".to_string()],
        &["no hate speech".to_string()], // different prohibition
    )
    .unwrap();

    let config = VrpAlignmentConfig {
        semantic_alignment_required: true,
        min_alignment_score: 0.1, // very low threshold — should still be Conflict
    };

    let status = compare_peer_anchor(&snap1, &snap2, &config);
    assert_eq!(status, VrpAlignmentStatus::Conflict);
}

#[test]
fn test_compare_peer_anchor_partial_when_prohibited_actions_match() {
    // Similar principles with matching prohibited actions should reach Partial.
    let snap1 = VrpAnchorSnapshot::new(
        &["free speech".to_string(), "privacy".to_string()],
        &["no doxxing".to_string()],
    )
    .unwrap();
    let snap2 = VrpAnchorSnapshot::new(
        &["free speech".to_string(), "transparency".to_string()],
        &["no doxxing".to_string()], // same prohibition
    )
    .unwrap();

    let config = VrpAlignmentConfig {
        semantic_alignment_required: true,
        min_alignment_score: 0.3,
    };

    let status = compare_peer_anchor(&snap1, &snap2, &config);
    assert_eq!(status, VrpAlignmentStatus::Partial);
}

#[test]
fn test_contracts_mutually_accepted() {
    let local = VrpCapabilitySharingContract {
        required_capabilities: vec!["cap_A".to_string()],
        offered_capabilities: vec!["cap_B".to_string()],
        redacted_topics: vec![],
    };
    let remote = VrpCapabilitySharingContract {
        required_capabilities: vec!["cap_B".to_string()],
        offered_capabilities: vec!["cap_A".to_string()],
        redacted_topics: vec![],
    };
    assert!(contracts_mutually_accepted(&local, &remote));

    let remote_lacking = VrpCapabilitySharingContract {
        required_capabilities: vec!["cap_B".to_string()],
        offered_capabilities: vec![], // Doesn't offer A
        redacted_topics: vec![],
    };
    assert!(!contracts_mutually_accepted(&local, &remote_lacking));

    let local_lacking = VrpCapabilitySharingContract {
        required_capabilities: vec!["cap_C".to_string()], // Wants C, remote doesn't offer
        offered_capabilities: vec!["cap_B".to_string()],
        redacted_topics: vec![],
    };
    assert!(!contracts_mutually_accepted(&local_lacking, &remote));
}

#[test]
fn test_resolve_transfer_scope() {
    let config_full = VrpTransferAcceptanceConfig {
        allow_full_knowledge: true,
        allow_reflection_summaries: true,
    };
    let config_partial = VrpTransferAcceptanceConfig {
        allow_full_knowledge: false,
        allow_reflection_summaries: true,
    };
    let config_none = VrpTransferAcceptanceConfig {
        allow_full_knowledge: false,
        allow_reflection_summaries: false,
    };

    assert_eq!(
        resolve_transfer_scope(VrpAlignmentStatus::Aligned, &config_full),
        VrpTransferScope::FullKnowledgeBundle
    );
    assert_eq!(
        resolve_transfer_scope(VrpAlignmentStatus::Aligned, &config_partial),
        VrpTransferScope::ReflectionSummariesOnly
    );
    assert_eq!(
        resolve_transfer_scope(VrpAlignmentStatus::Partial, &config_full),
        VrpTransferScope::ReflectionSummariesOnly
    );
    assert_eq!(
        resolve_transfer_scope(VrpAlignmentStatus::Conflict, &config_full),
        VrpTransferScope::NoTransfer
    );
    assert_eq!(
        resolve_transfer_scope(VrpAlignmentStatus::Aligned, &config_none),
        VrpTransferScope::NoTransfer
    );
}

#[test]
fn test_validate_federation_handshake_success() {
    let principles = vec!["p1".to_string()];
    let prohibited = vec!["no1".to_string()];
    let local_anchor = VrpAnchorSnapshot::new(&principles, &prohibited).unwrap();
    let local_contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
        redacted_topics: vec![],
    };

    let handshake = VrpFederationHandshake {
        anchor_snapshot: local_anchor.clone(),
        capability_contract: local_contract.clone(),
    };

    let align_config = VrpAlignmentConfig {
        semantic_alignment_required: false,
        min_alignment_score: 0.8,
    };
    let transfer_config = VrpTransferAcceptanceConfig {
        allow_full_knowledge: true,
        allow_reflection_summaries: true,
    };

    let report = validate_federation_handshake(
        &local_anchor,
        &local_contract,
        &handshake,
        &align_config,
        &transfer_config,
    );

    assert_eq!(report.alignment_status, VrpAlignmentStatus::Aligned);
    assert_eq!(report.transfer_scope, VrpTransferScope::FullKnowledgeBundle);
    assert!(report.negotiation_notes.is_empty());
}

#[test]
fn test_validate_federation_handshake_conflict_principles() {
    let local_anchor = VrpAnchorSnapshot::new(&["A".to_string()], &[]).unwrap();
    let remote_anchor = VrpAnchorSnapshot::new(&["B".to_string()], &[]).unwrap();
    let local_contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
        redacted_topics: vec![],
    };

    let handshake = VrpFederationHandshake {
        anchor_snapshot: remote_anchor,
        capability_contract: local_contract.clone(),
    };

    let align_config = VrpAlignmentConfig {
        semantic_alignment_required: false,
        min_alignment_score: 0.8,
    };
    let transfer_config = VrpTransferAcceptanceConfig {
        allow_full_knowledge: true,
        allow_reflection_summaries: true,
    };

    let report = validate_federation_handshake(
        &local_anchor,
        &local_contract,
        &handshake,
        &align_config,
        &transfer_config,
    );

    assert_eq!(report.alignment_status, VrpAlignmentStatus::Conflict);
    assert_eq!(report.transfer_scope, VrpTransferScope::NoTransfer);
}

#[test]
fn test_validate_federation_handshake_contract_fail() {
    let local_anchor = VrpAnchorSnapshot::new(&["A".to_string()], &[]).unwrap();
    let local_contract = VrpCapabilitySharingContract {
        required_capabilities: vec!["MustHave".to_string()],
        offered_capabilities: vec![],
        redacted_topics: vec![],
    };
    // Remote doesn't offer MustHave
    let remote_contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
        redacted_topics: vec![],
    };

    let handshake = VrpFederationHandshake {
        anchor_snapshot: local_anchor.clone(),
        capability_contract: remote_contract,
    };

    let align_config = VrpAlignmentConfig {
        semantic_alignment_required: false,
        min_alignment_score: 0.8,
    };
    let transfer_config = VrpTransferAcceptanceConfig {
        allow_full_knowledge: true,
        allow_reflection_summaries: true,
    };

    let report = validate_federation_handshake(
        &local_anchor,
        &local_contract,
        &handshake,
        &align_config,
        &transfer_config,
    );

    // Principles align, but contracts fail -> Conflict
    assert_eq!(report.alignment_status, VrpAlignmentStatus::Conflict);
    assert_eq!(report.transfer_scope, VrpTransferScope::NoTransfer);
    assert!(!report.negotiation_notes.is_empty());
    assert!(report.negotiation_notes[0].contains("Capability contracts incompatible"));
}

#[test]
fn test_check_transfer_acceptance() {
    let report_aligned = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Aligned,
        transfer_scope: VrpTransferScope::FullKnowledgeBundle,
        alignment_score: 1.0,
        negotiation_notes: vec![],
    };

    let report_partial = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Partial,
        transfer_scope: VrpTransferScope::ReflectionSummariesOnly,
        alignment_score: 0.5,
        negotiation_notes: vec![],
    };

    let report_conflict = VrpValidationReport {
        alignment_status: VrpAlignmentStatus::Conflict,
        transfer_scope: VrpTransferScope::NoTransfer,
        alignment_score: 0.0,
        negotiation_notes: vec![],
    };

    // 1. Conflict always fails
    assert!(matches!(
        check_transfer_acceptance(&report_conflict, VrpTransferScope::NoTransfer),
        Err(VrpTransferAcceptanceError::Conflict)
    ));

    // 2. Insufficient scope
    assert!(matches!(
        check_transfer_acceptance(&report_partial, VrpTransferScope::FullKnowledgeBundle),
        Err(VrpTransferAcceptanceError::Rejected(_))
    ));

    // 3. Sufficient scope (equal)
    assert!(
        check_transfer_acceptance(&report_partial, VrpTransferScope::ReflectionSummariesOnly)
            .is_ok()
    );

    // 4. Sufficient scope (greater)
    assert!(
        check_transfer_acceptance(&report_aligned, VrpTransferScope::ReflectionSummariesOnly)
            .is_ok()
    );

    // 5. NoTransfer requirement always met if not conflict
    assert!(check_transfer_acceptance(&report_partial, VrpTransferScope::NoTransfer).is_ok());
}

#[test]
fn test_redacted_topics_backward_compatible_deserialization() {
    // Contracts serialized without redacted_topics should still deserialize
    let json = r#"{"required_capabilities":["cap1"],"offered_capabilities":["cap2"]}"#;
    let contract: VrpCapabilitySharingContract = serde_json::from_str(json).unwrap();
    assert!(contract.redacted_topics.is_empty());
}

#[test]
fn test_redacted_topics_round_trip() {
    let contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
        redacted_topics: vec!["politics".to_string(), "finance".to_string()],
    };
    let json = serde_json::to_string(&contract).unwrap();
    let deserialized: VrpCapabilitySharingContract = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.redacted_topics, vec!["politics", "finance"]);
}

#[test]
fn test_anchor_snapshot_new_returns_ok_with_valid_system_clock() {
    // On any correctly configured system, new() should succeed and produce
    // a non-zero timestamp.
    let result = VrpAnchorSnapshot::new(&["p1".to_string()], &[]);
    assert!(
        result.is_ok(),
        "VrpAnchorSnapshot::new() should succeed on a valid system"
    );
    let snapshot = result.unwrap();
    assert!(snapshot.timestamp > 0, "timestamp should be non-zero");
    assert!(!snapshot.principles_hash.is_empty());
    assert!(!snapshot.prohibited_actions_hash.is_empty());
}

#[test]
fn test_anchor_snapshot_new_returns_ok_with_empty_inputs() {
    let result = VrpAnchorSnapshot::new(&[], &[]);
    assert!(result.is_ok());
    let snapshot = result.unwrap();
    // Even with empty inputs, hashes should be deterministic non-empty strings
    assert!(!snapshot.principles_hash.is_empty());
    assert!(!snapshot.prohibited_actions_hash.is_empty());
}

#[test]
fn test_vrp_error_display() {
    let err = VrpError::SystemClockInvalid;
    let msg = err.to_string();
    assert!(
        msg.contains("UNIX epoch"),
        "Error message should mention UNIX epoch, got: {msg}"
    );
}

fn report(status: VrpAlignmentStatus, scope: VrpTransferScope, score: f32) -> VrpValidationReport {
    VrpValidationReport {
        alignment_status: status,
        transfer_scope: scope,
        alignment_score: score,
        negotiation_notes: vec![],
    }
}

fn full_accept() -> VrpTransferAcceptanceConfig {
    VrpTransferAcceptanceConfig {
        allow_reflection_summaries: true,
        allow_full_knowledge: true,
    }
}

#[test]
fn reputation_gate_is_noop_when_healthy() {
    let r = report(
        VrpAlignmentStatus::Aligned,
        VrpTransferScope::FullKnowledgeBundle,
        1.0,
    );
    let out = apply_reputation_gate(r, 0.6, &full_accept());
    assert_eq!(out.alignment_status, VrpAlignmentStatus::Aligned);
    assert!(out.negotiation_notes.is_empty());
}

#[test]
fn reputation_gate_threshold_is_inclusive() {
    // Exactly at the threshold counts as healthy (>=) — no downgrade.
    let r = report(
        VrpAlignmentStatus::Aligned,
        VrpTransferScope::FullKnowledgeBundle,
        1.0,
    );
    let out = apply_reputation_gate(r, MIN_REPUTATION_FOR_FULL_ALIGNMENT, &full_accept());
    assert_eq!(out.alignment_status, VrpAlignmentStatus::Aligned);
}

#[test]
fn reputation_gate_downgrades_aligned_to_partial_when_poor() {
    let r = report(
        VrpAlignmentStatus::Aligned,
        VrpTransferScope::FullKnowledgeBundle,
        1.0,
    );
    let out = apply_reputation_gate(r, 0.1, &full_accept());
    assert_eq!(out.alignment_status, VrpAlignmentStatus::Partial);
    assert_eq!(
        out.transfer_scope,
        VrpTransferScope::ReflectionSummariesOnly
    );
    // The measured similarity is preserved — only the verdict is downgraded.
    assert_eq!(out.alignment_score, 1.0);
    assert!(out
        .negotiation_notes
        .iter()
        .any(|n| n.contains("downgraded")));
}

#[test]
fn reputation_gate_downgrades_partial_to_conflict_and_conflict_is_terminal() {
    let cfg = VrpTransferAcceptanceConfig {
        allow_reflection_summaries: true,
        allow_full_knowledge: false,
    };
    let partial = report(
        VrpAlignmentStatus::Partial,
        VrpTransferScope::ReflectionSummariesOnly,
        0.5,
    );
    let out = apply_reputation_gate(partial, 0.0, &cfg);
    assert_eq!(out.alignment_status, VrpAlignmentStatus::Conflict);
    assert_eq!(out.transfer_scope, VrpTransferScope::NoTransfer);

    // Conflict cannot be downgraded further.
    let conflict = report(
        VrpAlignmentStatus::Conflict,
        VrpTransferScope::NoTransfer,
        0.0,
    );
    let out2 = apply_reputation_gate(conflict, 0.0, &cfg);
    assert_eq!(out2.alignment_status, VrpAlignmentStatus::Conflict);
    assert!(out2.negotiation_notes.is_empty());
}

#[test]
fn compare_peer_anchor_scored_returns_measured_similarity() {
    let cfg = VrpAlignmentConfig {
        semantic_alignment_required: true,
        min_alignment_score: 0.8,
    };

    // Exact match → status Aligned, score 1.0.
    let a = VrpAnchorSnapshot::new(&["p1".to_string()], &["no1".to_string()]).unwrap();
    let (status, score) = compare_peer_anchor_scored(&a, &a.clone(), &cfg);
    assert_eq!(status, VrpAlignmentStatus::Aligned);
    assert_eq!(score, 1.0);

    // Prohibited-action divergence → Conflict, score 0.0 (no continuous score).
    let b = VrpAnchorSnapshot::new(&["p1".to_string()], &["no2".to_string()]).unwrap();
    let (status2, score2) = compare_peer_anchor_scored(&a, &b, &cfg);
    assert_eq!(status2, VrpAlignmentStatus::Conflict);
    assert_eq!(score2, 0.0);

    // Matching prohibitions, differing-but-overlapping principles → the real
    // cosine value is surfaced (NOT a hardcoded 0.5/1.0/0.0), and it is in
    // (0.0, 1.0). With a high threshold this is a Conflict that still reports
    // its measured similarity.
    let c = VrpAnchorSnapshot::new(
        &["free speech".to_string(), "privacy".to_string()],
        &["no doxxing".to_string()],
    )
    .unwrap();
    let d = VrpAnchorSnapshot::new(
        &["free speech".to_string(), "transparency".to_string()],
        &["no doxxing".to_string()],
    )
    .unwrap();
    let (status3, score3) = compare_peer_anchor_scored(&c, &d, &cfg);
    // Conflict under the 0.8 threshold, but the score is the real measured
    // cosine — the old placeholder would have reported exactly 0.0 for Conflict.
    assert_eq!(status3, VrpAlignmentStatus::Conflict);
    assert!(
        score3 > 0.0 && score3 < 1.0,
        "expected a measured cosine in (0,1), got {score3}"
    );
}

/// A server running the shipped defaults must be able to admit an agent.
///
/// `ServerPolicy::default()` has `principles: []` and
/// `agent_min_alignment_score: 0.8`, and the agent handshake hardcodes
/// `semantic_alignment_required: true`. Before this, an empty local list
/// made the semantic branch unreachable and control fell through to
/// `Conflict` — so the only agent a stock server could admit was one with
/// no principles and no prohibited actions at all. Agent participation, a
/// headline feature, did not work out of the box and said nothing about
/// why.
mod empty_local_anchor {
    use crate::{
        compare_peer_anchor_scored, VrpAlignmentConfig, VrpAlignmentStatus, VrpAnchorSnapshot,
    };

    fn anchor(principles: &[&str], prohibited: &[&str]) -> VrpAnchorSnapshot {
        VrpAnchorSnapshot::new(
            &principles.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            &prohibited.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
        .expect("anchor")
    }

    fn strict() -> VrpAlignmentConfig {
        VrpAlignmentConfig {
            semantic_alignment_required: true,
            min_alignment_score: 0.8,
        }
    }

    #[test]
    fn an_agent_with_principles_is_admitted_when_the_server_declares_none() {
        let local = anchor(&[], &[]);
        let remote = anchor(&["users deserve privacy"], &[]);

        let (status, _) = compare_peer_anchor_scored(&local, &remote, &strict());
        assert_eq!(
            status,
            VrpAlignmentStatus::Aligned,
            "a server that has declared no values rejected an agent for \
             having some — there was nothing for it to conflict with",
        );
    }

    #[test]
    fn prohibited_actions_are_still_enforced_with_no_local_principles() {
        // The one boundary the operator DID state must keep working. This is
        // the half that must not be weakened by admitting the case above.
        let local = anchor(&[], &["no surveillance"]);
        let remote = anchor(&["users deserve privacy"], &[]);

        let (status, _) = compare_peer_anchor_scored(&local, &remote, &strict());
        assert_eq!(
            status,
            VrpAlignmentStatus::Conflict,
            "a declared prohibition was ignored",
        );
    }

    #[test]
    fn a_server_with_principles_still_scores_them() {
        // The threshold has to remain live where the operator has actually
        // configured it, or this fix would have silently disabled the gate
        // everywhere instead of only where it could not run.
        let local = anchor(&["users deserve privacy"], &[]);
        let remote = anchor(&["maximise engagement at any cost"], &[]);

        let (status, score) = compare_peer_anchor_scored(&local, &remote, &strict());
        assert_eq!(status, VrpAlignmentStatus::Conflict);
        assert!(score < 0.8, "unrelated principles scored {score}");
    }

    #[test]
    fn identical_anchors_are_still_an_exact_match() {
        let local = anchor(&["be kind"], &["no spam"]);
        let remote = anchor(&["be kind"], &["no spam"]);

        let (status, score) = compare_peer_anchor_scored(&local, &remote, &strict());
        assert_eq!(status, VrpAlignmentStatus::Aligned);
        assert_eq!(score, 1.0);
    }
}
