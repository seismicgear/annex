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
        },
        capability_contract: VrpCapabilitySharingContract {
            required_capabilities: vec!["cap1".to_string()],
            offered_capabilities: vec!["cap2".to_string()],
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

    let snap1 = VrpAnchorSnapshot::new(&p1, &prohibited);
    let snap2 = VrpAnchorSnapshot::new(&p2, &prohibited);

    assert_eq!(snap1.principles_hash, snap2.principles_hash);
    assert_eq!(snap1.prohibited_actions_hash, snap2.prohibited_actions_hash);
    // Timestamps might differ slightly, so ignore them for hash check
}

#[test]
fn test_compare_peer_anchor_aligned() {
    let principles = vec!["p1".to_string()];
    let prohibited = vec!["no1".to_string()];
    let snap1 = VrpAnchorSnapshot::new(&principles, &prohibited);
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
    let snap1 = VrpAnchorSnapshot::new(&["p1".to_string()], &[]);
    let snap2 = VrpAnchorSnapshot::new(&["p2".to_string()], &[]); // different

    let config = VrpAlignmentConfig {
        semantic_alignment_required: false,
        min_alignment_score: 0.8,
    };

    let status = compare_peer_anchor(&snap1, &snap2, &config);
    assert_eq!(status, VrpAlignmentStatus::Conflict);
}

#[test]
fn test_contracts_mutually_accepted() {
    let local = VrpCapabilitySharingContract {
        required_capabilities: vec!["cap_A".to_string()],
        offered_capabilities: vec!["cap_B".to_string()],
    };
    let remote = VrpCapabilitySharingContract {
        required_capabilities: vec!["cap_B".to_string()],
        offered_capabilities: vec!["cap_A".to_string()],
    };
    assert!(contracts_mutually_accepted(&local, &remote));

    let remote_lacking = VrpCapabilitySharingContract {
        required_capabilities: vec!["cap_B".to_string()],
        offered_capabilities: vec![], // Doesn't offer A
    };
    assert!(!contracts_mutually_accepted(&local, &remote_lacking));

    let local_lacking = VrpCapabilitySharingContract {
        required_capabilities: vec!["cap_C".to_string()], // Wants C, remote doesn't offer
        offered_capabilities: vec!["cap_B".to_string()],
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
    let local_anchor = VrpAnchorSnapshot::new(&principles, &prohibited);
    let local_contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
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
    let local_anchor = VrpAnchorSnapshot::new(&["A".to_string()], &[]);
    let remote_anchor = VrpAnchorSnapshot::new(&["B".to_string()], &[]);
    let local_contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
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
    let local_anchor = VrpAnchorSnapshot::new(&["A".to_string()], &[]);
    let local_contract = VrpCapabilitySharingContract {
        required_capabilities: vec!["MustHave".to_string()],
        offered_capabilities: vec![],
    };
    // Remote doesn't offer MustHave
    let remote_contract = VrpCapabilitySharingContract {
        required_capabilities: vec![],
        offered_capabilities: vec![],
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
