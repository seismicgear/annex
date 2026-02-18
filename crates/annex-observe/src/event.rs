//! Event domain, payload, and record types for the public event log.

use serde::{Deserialize, Serialize};

/// Observability event domains.
///
/// Each domain groups related event types for filtering and auditing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventDomain {
    /// Identity operations: registrations, verifications, pseudonym derivations.
    #[serde(rename = "IDENTITY")]
    Identity,
    /// Presence graph changes: node additions, pruning, reactivation.
    #[serde(rename = "PRESENCE")]
    Presence,
    /// Federation lifecycle: established, realigned, severed.
    #[serde(rename = "FEDERATION")]
    Federation,
    /// Agent lifecycle: connected, realigned, disconnected.
    #[serde(rename = "AGENT")]
    Agent,
    /// Moderation actions taken by server operators or moderators.
    #[serde(rename = "MODERATION")]
    Moderation,
}

impl EventDomain {
    /// Returns the canonical string label for this domain.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Identity => "IDENTITY",
            Self::Presence => "PRESENCE",
            Self::Federation => "FEDERATION",
            Self::Agent => "AGENT",
            Self::Moderation => "MODERATION",
        }
    }
}

impl std::fmt::Display for EventDomain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EventDomain {
    type Err = ParseEventDomainError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "IDENTITY" => Ok(Self::Identity),
            "PRESENCE" => Ok(Self::Presence),
            "FEDERATION" => Ok(Self::Federation),
            "AGENT" => Ok(Self::Agent),
            "MODERATION" => Ok(Self::Moderation),
            _ => Err(ParseEventDomainError(s.to_string())),
        }
    }
}

/// Error returned when parsing an unknown event domain string.
#[derive(Debug, Clone)]
pub struct ParseEventDomainError(pub String);

impl std::fmt::Display for ParseEventDomainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown event domain: {}", self.0)
    }
}

impl std::error::Error for ParseEventDomainError {}

/// Structured event payloads for each event type.
///
/// Payloads are serialised to JSON and stored in the `payload_json` column
/// of the `public_event_log` table. Each variant corresponds to an
/// `event_type` string.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "SCREAMING_SNAKE_CASE")]
pub enum EventPayload {
    // ── Identity domain ──────────────────────────────────────────────
    /// A new identity commitment was registered in the Merkle tree.
    IdentityRegistered {
        /// The hex-encoded commitment.
        commitment_hex: String,
        /// The role code of the registrant.
        role_code: u8,
    },

    /// A zero-knowledge membership proof was verified.
    IdentityVerified {
        /// The hex-encoded commitment whose membership was proved.
        commitment_hex: String,
        /// The VRP topic against which verification was performed.
        topic: String,
    },

    /// A pseudonym was derived for a verified commitment.
    PseudonymDerived {
        /// The derived pseudonym identifier.
        pseudonym_id: String,
        /// The VRP topic used for derivation.
        topic: String,
    },

    // ── Presence domain ──────────────────────────────────────────────
    /// A new node was added to the presence graph.
    NodeAdded {
        /// The pseudonym of the new node.
        pseudonym_id: String,
        /// The type of participant (HUMAN, AI_AGENT, etc.).
        node_type: String,
    },

    /// A node was pruned from the presence graph due to inactivity.
    NodePruned {
        /// The pseudonym of the pruned node.
        pseudonym_id: String,
    },

    /// A previously pruned node was reactivated.
    NodeReactivated {
        /// The pseudonym of the reactivated node.
        pseudonym_id: String,
    },

    // ── Federation domain ────────────────────────────────────────────
    /// A new federation agreement was established with a remote server.
    FederationEstablished {
        /// The base URL of the remote instance.
        remote_url: String,
        /// The negotiated alignment status.
        alignment_status: String,
    },

    /// An existing federation agreement was realigned after policy change.
    FederationRealigned {
        /// The base URL of the remote instance.
        remote_url: String,
        /// The new alignment status.
        alignment_status: String,
        /// The previous alignment status.
        previous_status: String,
    },

    /// A federation agreement was severed.
    FederationSevered {
        /// The base URL of the remote instance.
        remote_url: String,
        /// The reason for severance.
        reason: String,
    },

    // ── Agent domain ─────────────────────────────────────────────────
    /// An agent completed VRP handshake and joined the server.
    AgentConnected {
        /// The agent's pseudonym.
        pseudonym_id: String,
        /// The alignment status from the VRP handshake.
        alignment_status: String,
    },

    /// An agent's alignment was re-evaluated (e.g., after policy change).
    AgentRealigned {
        /// The agent's pseudonym.
        pseudonym_id: String,
        /// The new alignment status.
        alignment_status: String,
        /// The previous alignment status.
        previous_status: String,
    },

    /// An agent was disconnected from the server.
    AgentDisconnected {
        /// The agent's pseudonym.
        pseudonym_id: String,
        /// The reason for disconnection.
        reason: String,
    },

    // ── Moderation domain ────────────────────────────────────────────
    /// A moderation action was performed.
    ModerationAction {
        /// The pseudonym of the moderator who performed the action.
        moderator_pseudonym: String,
        /// The type of action (e.g., "kick", "ban", "mute", "delete_message").
        action_type: String,
        /// The pseudonym of the target (if applicable).
        target_pseudonym: Option<String>,
        /// A human-readable description of the action.
        description: String,
    },
}

impl EventPayload {
    /// Returns the canonical event type string for this payload.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::IdentityRegistered { .. } => "IDENTITY_REGISTERED",
            Self::IdentityVerified { .. } => "IDENTITY_VERIFIED",
            Self::PseudonymDerived { .. } => "PSEUDONYM_DERIVED",
            Self::NodeAdded { .. } => "NODE_ADDED",
            Self::NodePruned { .. } => "NODE_PRUNED",
            Self::NodeReactivated { .. } => "NODE_REACTIVATED",
            Self::FederationEstablished { .. } => "FEDERATION_ESTABLISHED",
            Self::FederationRealigned { .. } => "FEDERATION_REALIGNED",
            Self::FederationSevered { .. } => "FEDERATION_SEVERED",
            Self::AgentConnected { .. } => "AGENT_CONNECTED",
            Self::AgentRealigned { .. } => "AGENT_REALIGNED",
            Self::AgentDisconnected { .. } => "AGENT_DISCONNECTED",
            Self::ModerationAction { .. } => "MODERATION_ACTION",
        }
    }

    /// Returns the entity type for this payload, used as the `entity_type`
    /// column in the event log.
    pub fn entity_type(&self) -> &'static str {
        match self {
            Self::IdentityRegistered { .. }
            | Self::IdentityVerified { .. }
            | Self::PseudonymDerived { .. } => "identity",
            Self::NodeAdded { .. } | Self::NodePruned { .. } | Self::NodeReactivated { .. } => {
                "node"
            }
            Self::FederationEstablished { .. }
            | Self::FederationRealigned { .. }
            | Self::FederationSevered { .. } => "federation",
            Self::AgentConnected { .. }
            | Self::AgentRealigned { .. }
            | Self::AgentDisconnected { .. } => "agent",
            Self::ModerationAction { .. } => "moderation",
        }
    }

    /// Returns the domain for this payload.
    pub fn domain(&self) -> EventDomain {
        match self {
            Self::IdentityRegistered { .. }
            | Self::IdentityVerified { .. }
            | Self::PseudonymDerived { .. } => EventDomain::Identity,
            Self::NodeAdded { .. } | Self::NodePruned { .. } | Self::NodeReactivated { .. } => {
                EventDomain::Presence
            }
            Self::FederationEstablished { .. }
            | Self::FederationRealigned { .. }
            | Self::FederationSevered { .. } => EventDomain::Federation,
            Self::AgentConnected { .. }
            | Self::AgentRealigned { .. }
            | Self::AgentDisconnected { .. } => EventDomain::Agent,
            Self::ModerationAction { .. } => EventDomain::Moderation,
        }
    }
}

/// A single row from the `public_event_log` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublicEvent {
    /// Auto-incremented row ID.
    pub id: i64,
    /// The server that owns this event.
    pub server_id: i64,
    /// The event domain (e.g., `IDENTITY`, `PRESENCE`).
    pub domain: String,
    /// The specific event type (e.g., `IDENTITY_REGISTERED`).
    pub event_type: String,
    /// The type of entity involved (e.g., `identity`, `node`).
    pub entity_type: String,
    /// The identifier of the entity involved.
    pub entity_id: String,
    /// Monotonically increasing sequence number within the server.
    pub seq: i64,
    /// The structured event payload as a JSON string.
    pub payload_json: String,
    /// ISO 8601 timestamp of when the event occurred.
    pub occurred_at: String,
}
