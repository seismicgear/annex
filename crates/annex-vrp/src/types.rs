use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

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

impl FromStr for VrpAlignmentStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ALIGNED" => Ok(VrpAlignmentStatus::Aligned),
            "PARTIAL" => Ok(VrpAlignmentStatus::Partial),
            "CONFLICT" => Ok(VrpAlignmentStatus::Conflict),
            _ => Err(format!("unknown alignment status: {}", s)),
        }
    }
}

/// Defines the scope of data transfer permitted based on alignment.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
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

impl FromStr for VrpTransferScope {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "NO_TRANSFER" => Ok(VrpTransferScope::NoTransfer),
            "REFLECTION_SUMMARIES_ONLY" => Ok(VrpTransferScope::ReflectionSummariesOnly),
            "FULL_KNOWLEDGE_BUNDLE" => Ok(VrpTransferScope::FullKnowledgeBundle),
            _ => Err(format!("unknown transfer scope: {}", s)),
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
    /// Original principle texts for semantic alignment. Empty when not available.
    #[serde(default)]
    pub principles: Vec<String>,
    /// Original prohibited action texts for semantic alignment. Empty when not available.
    #[serde(default)]
    pub prohibited_actions: Vec<String>,
}

/// A contract defining required and offered capabilities for an interaction.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VrpCapabilitySharingContract {
    /// Capabilities required by this entity from the counterparty.
    pub required_capabilities: Vec<String>,
    /// Capabilities offered by this entity to the counterparty.
    pub offered_capabilities: Vec<String>,
    /// Knowledge domains that must not be shared in RTX transfers.
    ///
    /// Bundles whose `domain_tags` overlap with these topics are blocked
    /// from transfer per the VRP agreement.
    #[serde(default)]
    pub redacted_topics: Vec<String>,
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

/// Errors that can occur during VRP operations.
#[derive(thiserror::Error, Debug)]
pub enum VrpError {
    /// The system clock returned a time before the UNIX epoch, making it
    /// impossible to generate a valid timestamp for the anchor snapshot.
    /// This indicates a fundamentally misconfigured host and cannot be
    /// recovered at the application layer.
    #[error("system clock returned a time before the UNIX epoch")]
    SystemClockInvalid,
}

/// Errors that can occur during transfer acceptance validation.
#[derive(thiserror::Error, Debug)]
pub enum VrpTransferAcceptanceError {
    #[error("Transfer rejected: {0}")]
    Rejected(String),
    #[error("Alignment conflict")]
    Conflict,
}
