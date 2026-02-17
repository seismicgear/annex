//! RTX (Recursive Thought Exchange) knowledge exchange for the Annex platform.
//!
//! Implements structured agent-to-agent knowledge transfer: reflection
//! summary bundles, publish/subscribe, transfer scope enforcement,
//! cross-server relay, and governance-mediated auditing.
//!
//! RTX enables distributed cognition across the Annex federation. An agent
//! on one server can package a reflection, publish it, and have it delivered
//! to aligned agents on federated servers — gated at every step by VRP
//! transfer scope and capability contracts.
//!
//! # Core types
//!
//! - [`ReflectionSummaryBundle`] — the atomic unit of knowledge transfer
//! - [`BundleProvenance`] — tracks relay path across federated servers
//! - [`RtxSubscription`] — agent subscription filters for bundle delivery
//!
//! # Transfer scope enforcement
//!
//! Every bundle delivery is gated by the VRP transfer scope negotiated
//! between sender and receiver (or their respective servers):
//!
//! - `NoTransfer` — bundle is not delivered
//! - `ReflectionSummariesOnly` — `reasoning_chain` is stripped before delivery
//! - `FullKnowledgeBundle` — all fields delivered intact
//!
//! # Redacted topics
//!
//! Agents may have redacted topics in their capability contracts. Bundles
//! whose `domain_tags` overlap with redacted topics are blocked from transfer.

pub mod error;
pub mod types;
pub mod validation;

pub use error::RtxError;
pub use types::{BundleProvenance, ReflectionSummaryBundle, RtxSubscription};
pub use validation::{
    bundle_signing_payload, check_redacted_topics, enforce_transfer_scope,
    validate_bundle_structure,
};

#[cfg(test)]
mod tests {
    use super::*;
    use annex_vrp::VrpTransferScope;

    /// Creates a test bundle with all fields populated.
    fn make_test_bundle() -> ReflectionSummaryBundle {
        ReflectionSummaryBundle {
            bundle_id: "bundle-001".to_string(),
            source_pseudonym: "agent-alpha".to_string(),
            source_server: "http://server-a.example.com".to_string(),
            domain_tags: vec!["rust".to_string(), "cryptography".to_string()],
            summary: "Poseidon hash performance can be improved with precomputed round constants."
                .to_string(),
            reasoning_chain: Some(
                "Observed Poseidon benchmarks → analyzed round constant table → identified \
                 caching opportunity → validated with 10K iterations."
                    .to_string(),
            ),
            caveats: vec!["Benchmark results are hardware-dependent.".to_string()],
            created_at: 1700000000000,
            signature: "abcdef0123456789".to_string(),
            vrp_handshake_ref: "1:10:42".to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // Serialization tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_bundle_serialization_round_trip() {
        let bundle = make_test_bundle();
        let json = serde_json::to_string(&bundle).unwrap();
        let deserialized: ReflectionSummaryBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(bundle, deserialized);
    }

    #[test]
    fn test_bundle_serialization_without_reasoning_chain() {
        let mut bundle = make_test_bundle();
        bundle.reasoning_chain = None;
        let json = serde_json::to_string(&bundle).unwrap();
        let deserialized: ReflectionSummaryBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.reasoning_chain, None);
    }

    #[test]
    fn test_bundle_json_fields_present() {
        let bundle = make_test_bundle();
        let value: serde_json::Value = serde_json::to_value(&bundle).unwrap();
        assert_eq!(value["bundle_id"], "bundle-001");
        assert_eq!(value["source_pseudonym"], "agent-alpha");
        assert_eq!(value["source_server"], "http://server-a.example.com");
        assert_eq!(value["domain_tags"][0], "rust");
        assert_eq!(value["domain_tags"][1], "cryptography");
        assert!(value["reasoning_chain"].is_string());
        assert_eq!(
            value["caveats"][0],
            "Benchmark results are hardware-dependent."
        );
        assert_eq!(value["created_at"].as_u64().unwrap(), 1700000000000u64);
        assert_eq!(value["signature"], "abcdef0123456789");
        assert_eq!(value["vrp_handshake_ref"], "1:10:42");
    }

    // -----------------------------------------------------------------------
    // Transfer scope enforcement tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_enforce_full_knowledge_bundle_preserves_reasoning_chain() {
        let bundle = make_test_bundle();
        let result = enforce_transfer_scope(&bundle, VrpTransferScope::FullKnowledgeBundle);
        let scoped = result.unwrap();
        assert!(scoped.reasoning_chain.is_some());
        assert_eq!(scoped, bundle);
    }

    #[test]
    fn test_enforce_reflection_summaries_only_strips_reasoning_chain() {
        let bundle = make_test_bundle();
        assert!(bundle.reasoning_chain.is_some());

        let result = enforce_transfer_scope(&bundle, VrpTransferScope::ReflectionSummariesOnly);
        let scoped = result.unwrap();
        assert!(scoped.reasoning_chain.is_none());
        // All other fields preserved
        assert_eq!(scoped.bundle_id, bundle.bundle_id);
        assert_eq!(scoped.summary, bundle.summary);
        assert_eq!(scoped.source_pseudonym, bundle.source_pseudonym);
        assert_eq!(scoped.domain_tags, bundle.domain_tags);
        assert_eq!(scoped.caveats, bundle.caveats);
        assert_eq!(scoped.signature, bundle.signature);
    }

    #[test]
    fn test_enforce_no_transfer_returns_error() {
        let bundle = make_test_bundle();
        let result = enforce_transfer_scope(&bundle, VrpTransferScope::NoTransfer);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RtxError::TransferDenied(_)));
    }

    #[test]
    fn test_enforce_reflection_summaries_on_bundle_without_reasoning_chain() {
        let mut bundle = make_test_bundle();
        bundle.reasoning_chain = None;
        let result = enforce_transfer_scope(&bundle, VrpTransferScope::ReflectionSummariesOnly);
        let scoped = result.unwrap();
        assert!(scoped.reasoning_chain.is_none());
    }

    // -----------------------------------------------------------------------
    // Redacted topics tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_no_redacted_topics_passes() {
        let bundle = make_test_bundle();
        let redacted: Vec<String> = vec![];
        assert!(check_redacted_topics(&bundle, &redacted).is_ok());
    }

    #[test]
    fn test_non_overlapping_redacted_topics_passes() {
        let bundle = make_test_bundle();
        let redacted = vec!["politics".to_string(), "finance".to_string()];
        assert!(check_redacted_topics(&bundle, &redacted).is_ok());
    }

    #[test]
    fn test_overlapping_redacted_topic_blocked() {
        let bundle = make_test_bundle();
        let redacted = vec!["cryptography".to_string()];
        let result = check_redacted_topics(&bundle, &redacted);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RtxError::RedactedTopic(ref t) if t == "cryptography"));
    }

    #[test]
    fn test_first_matching_redacted_topic_is_reported() {
        let bundle = make_test_bundle();
        // "rust" appears before "cryptography" in domain_tags
        let redacted = vec!["rust".to_string(), "cryptography".to_string()];
        let result = check_redacted_topics(&bundle, &redacted);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, RtxError::RedactedTopic(ref t) if t == "rust"));
    }

    // -----------------------------------------------------------------------
    // Bundle validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_valid_bundle_passes_validation() {
        let bundle = make_test_bundle();
        assert!(validate_bundle_structure(&bundle).is_ok());
    }

    #[test]
    fn test_empty_bundle_id_fails() {
        let mut bundle = make_test_bundle();
        bundle.bundle_id = String::new();
        let result = validate_bundle_structure(&bundle);
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), RtxError::InvalidBundle(ref m) if m.contains("bundle_id"))
        );
    }

    #[test]
    fn test_empty_source_pseudonym_fails() {
        let mut bundle = make_test_bundle();
        bundle.source_pseudonym = String::new();
        let result = validate_bundle_structure(&bundle);
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), RtxError::InvalidBundle(ref m) if m.contains("source_pseudonym"))
        );
    }

    #[test]
    fn test_empty_source_server_fails() {
        let mut bundle = make_test_bundle();
        bundle.source_server = String::new();
        let result = validate_bundle_structure(&bundle);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_summary_fails() {
        let mut bundle = make_test_bundle();
        bundle.summary = String::new();
        let result = validate_bundle_structure(&bundle);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_signature_fails() {
        let mut bundle = make_test_bundle();
        bundle.signature = String::new();
        let result = validate_bundle_structure(&bundle);
        assert!(result.is_err());
    }

    #[test]
    fn test_empty_vrp_handshake_ref_fails() {
        let mut bundle = make_test_bundle();
        bundle.vrp_handshake_ref = String::new();
        let result = validate_bundle_structure(&bundle);
        assert!(result.is_err());
    }

    #[test]
    fn test_zero_created_at_fails() {
        let mut bundle = make_test_bundle();
        bundle.created_at = 0;
        let result = validate_bundle_structure(&bundle);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Signing payload tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_signing_payload_deterministic() {
        let bundle = make_test_bundle();
        let payload1 = bundle_signing_payload(&bundle);
        let payload2 = bundle_signing_payload(&bundle);
        assert_eq!(payload1, payload2);
    }

    #[test]
    fn test_signing_payload_changes_with_fields() {
        let bundle = make_test_bundle();
        let payload_original = bundle_signing_payload(&bundle);

        let mut modified = bundle.clone();
        modified.summary = "Different summary".to_string();
        let payload_modified = bundle_signing_payload(&modified);

        assert_ne!(payload_original, payload_modified);
    }

    #[test]
    fn test_signing_payload_includes_all_required_fields() {
        let bundle = make_test_bundle();
        let payload = bundle_signing_payload(&bundle);
        assert!(payload.contains(&bundle.bundle_id));
        assert!(payload.contains(&bundle.source_pseudonym));
        assert!(payload.contains(&bundle.source_server));
        assert!(payload.contains(&bundle.summary));
        assert!(payload.contains(&bundle.created_at.to_string()));
    }

    // -----------------------------------------------------------------------
    // Provenance tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_provenance_serialization() {
        let provenance = BundleProvenance {
            origin_server: "http://server-a.example.com".to_string(),
            relay_path: vec![
                "http://server-b.example.com".to_string(),
                "http://server-c.example.com".to_string(),
            ],
            bundle_id: "bundle-001".to_string(),
        };

        let json = serde_json::to_string(&provenance).unwrap();
        let deserialized: BundleProvenance = serde_json::from_str(&json).unwrap();
        assert_eq!(provenance, deserialized);
        assert_eq!(deserialized.relay_path.len(), 2);
    }

    // -----------------------------------------------------------------------
    // Subscription tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_subscription_serialization() {
        let sub = RtxSubscription {
            subscriber_pseudonym: "agent-beta".to_string(),
            domain_filters: vec!["rust".to_string(), "security".to_string()],
            accept_federated: true,
        };

        let json = serde_json::to_string(&sub).unwrap();
        let deserialized: RtxSubscription = serde_json::from_str(&json).unwrap();
        assert_eq!(sub, deserialized);
    }

    // -----------------------------------------------------------------------
    // Integration: scope enforcement + validation combined
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_then_enforce_full_scope() {
        let bundle = make_test_bundle();
        validate_bundle_structure(&bundle).unwrap();
        let scoped =
            enforce_transfer_scope(&bundle, VrpTransferScope::FullKnowledgeBundle).unwrap();
        assert!(scoped.reasoning_chain.is_some());
    }

    #[test]
    fn test_validate_then_enforce_reflection_scope() {
        let bundle = make_test_bundle();
        validate_bundle_structure(&bundle).unwrap();
        let scoped =
            enforce_transfer_scope(&bundle, VrpTransferScope::ReflectionSummariesOnly).unwrap();
        assert!(scoped.reasoning_chain.is_none());
        // Scoped bundle should still pass validation
        validate_bundle_structure(&scoped).unwrap();
    }

    #[test]
    fn test_validate_then_check_redacted_then_enforce() {
        let bundle = make_test_bundle();
        validate_bundle_structure(&bundle).unwrap();
        check_redacted_topics(&bundle, &["finance".to_string()]).unwrap();
        let scoped =
            enforce_transfer_scope(&bundle, VrpTransferScope::ReflectionSummariesOnly).unwrap();
        assert!(scoped.reasoning_chain.is_none());
    }
}
