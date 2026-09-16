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

/// One server's signed statement that it forwarded this bundle.
///
/// A hop is not a string. `relay_path` was a `Vec<String>` any relayer could
/// rewrite freely — remove itself, invent an upstream, claim a path it was never
/// on — and it was reset to `vec![local_public_url]` on every send, so it could
/// never grow past one entry in the first place. Each hop now signs its own
/// place in the chain, and the signature covers the PREVIOUS hop's payload
/// digest, so a chain is a chain rather than a bag of independent claims: a
/// relayer cannot reorder, drop or splice hops without invalidating every
/// signature after the edit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RelayHop {
    /// The public URL of the server that forwarded the bundle at this hop.
    pub server: String,
    /// `rtx_bundle_content_hash` of the bundle as this hop sent it.
    ///
    /// Per-hop rather than once for the chain, because transfer-scope
    /// enforcement rewrites the bundle in flight: a hop that strips a reasoning
    /// chain for a `ReflectionSummariesOnly` peer sends different bytes than it
    /// received, and both are legitimate.
    pub content_hash: String,
    /// Hex Ed25519 signature over `relay_hop_payload(..)` for this hop.
    pub signature: String,
}

/// The origin's signed statement about a bundle, which no relayer can forge.
///
/// Every hop signature is made by a relayer, so a chain of them proves only
/// that some servers handled the bundle — not that the server named as the
/// origin ever published it. Without this, "B relayed A's bundle" and "B wrote a
/// bundle and put A's name on it" are the same envelope.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OriginAttestation {
    /// Hex Ed25519 signature over `origin_attestation_payload(..)`, made with
    /// the origin server's instance signing key.
    pub signature: String,
    /// `SHA-256` of the bundle's reasoning chain as the origin published it, or
    /// of the empty string when there was none.
    ///
    /// The origin cannot sign the content hash a receiver computes, because
    /// scope enforcement legitimately strips the reasoning chain in flight. So
    /// the origin signs a digest over everything that survives stripping, plus
    /// this commitment to the part that does not. A relayer can therefore
    /// REMOVE a reasoning chain (that is the policy working) but cannot ADD or
    /// alter one the origin never wrote.
    pub reasoning_commitment: String,
    /// The furthest the origin is willing for this bundle to travel, counted in
    /// hops. Bounded by the receiving operator's own ceiling — see
    /// `RTX_HOP_CEILING`. A publisher limiting its own blast radius and an
    /// operator limiting what it will carry are different questions and both
    /// get an answer.
    pub max_hops: u8,
}

/// The provenance chain for a relayed RTX bundle.
///
/// When a bundle is relayed across federated servers, each hop appends a signed
/// [`RelayHop`]. This preserves the full provenance chain from original source
/// to final destination, and — unlike the plain string list it replaces — makes
/// each claim in that chain attributable.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BundleProvenance {
    /// The original source server where the bundle was created.
    pub origin_server: String,
    /// Ordered list of servers that relayed this bundle.
    ///
    /// **Deprecated in favour of `hops`**, and still populated as
    /// `hops.iter().map(|h| h.server)` so a peer on an older build keeps
    /// parsing the envelope. Do not read it for a trust decision: nothing
    /// signs it.
    pub relay_path: Vec<String>,
    /// The bundle ID this provenance tracks.
    pub bundle_id: String,
    /// The signed hop chain. Empty for a bundle that has not been relayed yet,
    /// and absent on an envelope from a peer that predates it.
    #[serde(default)]
    pub hops: Vec<RelayHop>,
    /// The origin's attestation. Absent on an envelope from a peer that
    /// predates it; a receiver configured with `rtx_require_hop_chain` refuses
    /// such an envelope rather than trusting the relayer's word for who wrote
    /// the bundle.
    #[serde(default)]
    pub origin: Option<OriginAttestation>,
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
