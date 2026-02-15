use serde::{Deserialize, Serialize};
use std::fmt;

/// Alignment status of a VRP handshake.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VrpAlignmentStatus {
    /// The counterparty is fully aligned with the local policy/ethics.
    Aligned,
    /// The counterparty is partially aligned; some interactions may be restricted.
    Partial,
    /// The counterparty is in conflict; most interactions are prohibited.
    Conflict,
}

impl fmt::Display for VrpAlignmentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VrpAlignmentStatus::Aligned => write!(f, "ALIGNED"),
            VrpAlignmentStatus::Partial => write!(f, "PARTIAL"),
            VrpAlignmentStatus::Conflict => write!(f, "CONFLICT"),
        }
    }
}

/// Defines the scope of data transfer permitted based on alignment.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum VrpTransferScope {
    /// No data transfer allowed.
    NoTransfer,
    /// Only high-level reflection summaries can be transferred.
    ReflectionSummariesOnly,
    /// Full knowledge bundles including reasoning chains can be transferred.
    FullKnowledgeBundle,
}

impl fmt::Display for VrpTransferScope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VrpTransferScope::NoTransfer => write!(f, "NO_TRANSFER"),
            VrpTransferScope::ReflectionSummariesOnly => write!(f, "REFLECTION_SUMMARIES_ONLY"),
            VrpTransferScope::FullKnowledgeBundle => write!(f, "FULL_KNOWLEDGE_BUNDLE"),
        }
    }
}

/// A snapshot of an entity's ethical or policy root for comparison.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VrpAnchorSnapshot {
    /// Hash of the principles list.
    pub principles_hash: String,
    /// Hash of the prohibited actions list.
    pub prohibited_actions_hash: String,
    /// Timestamp of when this snapshot was generated.
    pub timestamp: u64,
}

/// A contract defining required and offered capabilities for an interaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VrpCapabilitySharingContract {
    /// Capabilities required by this entity from the counterparty.
    pub required_capabilities: Vec<String>,
    /// Capabilities offered by this entity to the counterparty.
    pub offered_capabilities: Vec<String>,
}

/// The payload exchanged during a VRP handshake.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VrpFederationHandshake {
    /// The sender's anchor snapshot.
    pub anchor_snapshot: VrpAnchorSnapshot,
    /// The sender's capability sharing contract.
    pub capability_contract: VrpCapabilitySharingContract,
}

/// Configuration for alignment evaluation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VrpAlignmentConfig {
    /// Whether semantic alignment (embedding comparison) is required.
    pub semantic_alignment_required: bool,
    /// Minimum numerical score (0.0 - 1.0) to be considered aligned.
    pub min_alignment_score: f32,
}

/// Configuration for transfer acceptance.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VrpTransferAcceptanceConfig {
    /// Whether reflection summaries are accepted.
    pub allow_reflection_summaries: bool,
    /// Whether full knowledge bundles are accepted.
    pub allow_full_knowledge: bool,
}

/// The result of a VRP validation/handshake process.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VrpValidationReport {
    /// The resulting alignment status.
    pub alignment_status: VrpAlignmentStatus,
    /// The negotiated transfer scope.
    pub transfer_scope: VrpTransferScope,
    /// The numerical alignment score computed.
    pub alignment_score: f32,
    /// Notes or reasons for the alignment outcome.
    pub negotiation_notes: Vec<String>,
}

/// Errors that can occur during transfer acceptance validation.
#[derive(thiserror::Error, Debug)]
pub enum VrpTransferAcceptanceError {
    #[error("Transfer rejected: {0}")]
    Rejected(String),
    #[error("Alignment conflict")]
    Conflict,
}
