use crate::types::*;

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
