//! Shared types, error definitions, and constants for the Annex platform.
//!
//! This crate provides the foundational types used across all Annex crates,
//! including domain-specific error types (via `thiserror`), participant role
//! codes, server configuration structures, and common constants.
//!
//! No crate in the workspace depends on anything *except* `annex-types` for
//! cross-cutting type definitions. This keeps the dependency graph clean and
//! prevents circular dependencies.

use serde::{Deserialize, Serialize};

/// Participant role codes as defined by the VRP identity model.
///
/// Each participant in the Annex network has a role code that is part of
/// their identity commitment: `commitment = Poseidon(sk, roleCode, nodeId)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RoleCode {
    /// A human participant.
    Human = 1,
    /// An AI agent participant.
    AiAgent = 2,
    /// A collective (group identity).
    Collective = 3,
    /// A federation bridge.
    Bridge = 4,
    /// A platform service.
    Service = 5,
}

impl RoleCode {
    /// Returns the numeric code for this role.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Attempts to convert a numeric code to a `RoleCode`.
    ///
    /// Returns `None` if the code does not correspond to a known role.
    pub fn from_u8(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Human),
            2 => Some(Self::AiAgent),
            3 => Some(Self::Collective),
            4 => Some(Self::Bridge),
            5 => Some(Self::Service),
            _ => None,
        }
    }

    /// Returns the string label for this role.
    pub fn label(self) -> &'static str {
        match self {
            Self::Human => "HUMAN",
            Self::AiAgent => "AI_AGENT",
            Self::Collective => "COLLECTIVE",
            Self::Bridge => "BRIDGE",
            Self::Service => "SERVICE",
        }
    }
}

/// VRP alignment status produced by `compare_peer_anchor`.
///
/// Determines the level of trust between two entities after VRP negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlignmentStatus {
    /// Full trust: principles and prohibitions match.
    Aligned,
    /// Partial trust: some overlap, no direct conflicts.
    Partial,
    /// No trust: direct opposition detected.
    Conflict,
}

/// VRP transfer scope — determines what knowledge can cross a trust boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferScope {
    /// No data crosses the boundary.
    NoTransfer,
    /// Only compressed summaries without raw reasoning chains.
    ReflectionSummariesOnly,
    /// Complete reflection bundles with full context.
    FullKnowledgeBundle,
}

/// Channel types supported by the Annex platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChannelType {
    /// Text-only channel.
    Text,
    /// Voice-only channel.
    Voice,
    /// Combined text and voice channel.
    Hybrid,
    /// Agent-dedicated channel (RTX delivery, agent coordination).
    Agent,
    /// One-way broadcast channel.
    Broadcast,
}

/// Federation scope for a channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FederationScope {
    /// Channel is local to this server only.
    Local,
    /// Channel is visible to federation peers.
    Federated,
}

/// Graph node types (mirrors `RoleCode` for the presence graph layer).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NodeType {
    /// A human participant node.
    Human,
    /// An AI agent node.
    AiAgent,
    /// A collective node.
    Collective,
    /// A federation bridge node.
    Bridge,
    /// A platform service node.
    Service,
}

/// Graph edge relationship types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EdgeKind {
    /// Participant is a member of a channel/group.
    MemberOf,
    /// Two participants have a direct connection.
    Connected,
    /// An agent is actively serving in a channel.
    AgentServing,
    /// Two servers are federated.
    FederatedWith,
    /// A participant moderates a channel.
    Moderates,
}

/// Capability flags for a participant.
///
/// These flags determine what actions a participant is allowed to perform
/// on the platform, independent of their role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Capabilities {
    /// Can join voice channels and publish audio.
    pub can_voice: bool,
    /// Can perform moderation actions (kick, ban, delete).
    pub can_moderate: bool,
    /// Can generate invite links.
    pub can_invite: bool,
    /// Can initiate federation handshakes.
    pub can_federate: bool,
    /// Can operate as a bridge.
    pub can_bridge: bool,
}

/// Visibility levels for the presence graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VisibilityLevel {
    /// The viewer is the participant themselves.
    Self_,
    /// Viewer is 1 degree away (direct connection).
    Degree1,
    /// Viewer is 2 degrees away.
    Degree2,
    /// Viewer is 3 degrees away.
    Degree3,
    /// Viewer is further away but on the same server (aggregate stats only).
    AggregateOnly,
    /// No visibility.
    None,
}

mod policy;
pub use policy::ServerPolicy;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_code_round_trip() {
        for code in [
            RoleCode::Human,
            RoleCode::AiAgent,
            RoleCode::Collective,
            RoleCode::Bridge,
            RoleCode::Service,
        ] {
            let n = code.as_u8();
            assert_eq!(RoleCode::from_u8(n), Some(code));
        }
    }

    #[test]
    fn role_code_invalid() {
        assert_eq!(RoleCode::from_u8(0), None);
        assert_eq!(RoleCode::from_u8(6), None);
        assert_eq!(RoleCode::from_u8(255), None);
    }

    #[test]
    fn role_code_labels() {
        assert_eq!(RoleCode::Human.label(), "HUMAN");
        assert_eq!(RoleCode::AiAgent.label(), "AI_AGENT");
        assert_eq!(RoleCode::Collective.label(), "COLLECTIVE");
        assert_eq!(RoleCode::Bridge.label(), "BRIDGE");
        assert_eq!(RoleCode::Service.label(), "SERVICE");
    }
}
