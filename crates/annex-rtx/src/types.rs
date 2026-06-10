//! Core types for the RTX (Recursive Thought Exchange) knowledge transfer system.
//!
//! The primary type is [`ReflectionSummaryBundle`], which represents a unit of
//! agent-to-agent knowledge exchange. Bundles are gated at every step by VRP
//! transfer scope and capability contracts.

use serde::{Deserialize, Serialize};

/// A unit of agent-to-agent knowledge exchange.
///
/// Reflection summary bundles are the atomic unit of RTX knowledge transfer.
/// An agent packages a reflection — a distilled insight, reasoning output, or
/// domain summary — into a bundle, signs it, and publishes it for delivery
/// to aligned agents on the same or federated servers.
///
/// Transfer scope determines what fields are included:
/// - `ReflectionSummariesOnly`: `reasoning_chain` is stripped before delivery.
/// - `FullKnowledgeBundle`: all fields are delivered intact.
/// - `NoTransfer`: bundle is not delivered at all.
///
/// # Trust model
///
/// The `signature` field carries an Ed25519 signature from the source
/// agent's key for downstream consumers that wish to verify it, but the
/// server's acceptance of a bundle does **not** rest on that per-agent
/// signature (agents do not register signing keys with the server). Authenticity
/// is instead bound at two enforced boundaries:
///
/// - **Local publish** (`POST /api/rtx/publish`): the handler rejects any
///   bundle whose `source_pseudonym` does not match the authenticated
///   caller's pseudonym, and requires an active agent registration with a
///   transfer scope that permits publishing. An agent therefore cannot
///   publish under another agent's identity.
/// - **Cross-server relay** (`POST /api/federation/rtx`): the receiving
///   server verifies the relaying server's Ed25519 signature over the
///   relay envelope against that instance's known public key, and requires
///   an active federation agreement with the origin server before
///   accepting the bundle. The relaying server vouches for the bundle it
///   accepted locally under the rule above.
///
/// The signed payload, when present, is:
/// `SHA256(bundle_id + source_pseudonym + source_server + summary + created_at)`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReflectionSummaryBundle {
    /// Unique identifier for this bundle (UUID v4).
    pub bundle_id: String,
    /// The pseudonym of the agent that produced this reflection.
    pub source_pseudonym: String,
    /// The base URL of the server where the source agent resides.
    pub source_server: String,
    /// Domain tags categorizing the reflection content (e.g., `["rust", "security"]`).
    pub domain_tags: Vec<String>,
    /// The distilled summary of the reflection.
    pub summary: String,
    /// The full reasoning chain that produced this reflection.
    ///
    /// Only included when the transfer scope is `FullKnowledgeBundle`.
    /// Automatically stripped when enforcing `ReflectionSummariesOnly` scope.
    pub reasoning_chain: Option<String>,
    /// Caveats, limitations, or confidence qualifiers for this reflection.
    pub caveats: Vec<String>,
    /// Creation timestamp in milliseconds since Unix epoch.
    pub created_at: u128,
    /// Ed25519 signature of the bundle payload (hex-encoded).
    ///
    /// Signed payload: `SHA256(bundle_id + source_pseudonym + source_server + summary + created_at)`.
    pub signature: String,
    /// Reference to the VRP handshake that authorized this transfer.
    ///
    /// Format: handshake log ID or `"server_id:remote_instance_id:agreement_id"`.
    pub vrp_handshake_ref: String,
}

/// The provenance chain for a relayed RTX bundle.
///
/// When a bundle is relayed across federated servers, each hop appends
/// its server URL to the relay path. This preserves the full provenance
/// chain from original source to final destination.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleProvenance {
    /// The original source server where the bundle was created.
    pub origin_server: String,
    /// Ordered list of servers that relayed this bundle.
    pub relay_path: Vec<String>,
    /// The bundle ID this provenance tracks.
    pub bundle_id: String,
}

/// An RTX topic subscription filter.
///
/// Agents subscribe to RTX bundles by specifying domain tag filters.
/// Bundles are delivered only if at least one of their `domain_tags`
/// matches at least one of the subscriber's `domain_filters`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RtxSubscription {
    /// The pseudonym of the subscribing agent.
    pub subscriber_pseudonym: String,
    /// Domain tags this agent is interested in.
    pub domain_filters: Vec<String>,
    /// Whether to accept bundles from federated servers (not just local).
    pub accept_federated: bool,
}
