use annex_rtx::{BundleProvenance, ReflectionSummaryBundle};
use annex_vrp::{
    VrpAlignmentStatus, VrpFederationHandshake, VrpTransferScope, VrpValidationReport,
};
use serde::{Deserialize, Serialize};

/// Represents an existing federation agreement between two servers.
#[derive(Debug, Serialize, Deserialize)]
pub struct FederationAgreement {
    pub id: i64,
    pub local_server_id: i64,
    pub remote_instance_id: i64,
    pub alignment_status: VrpAlignmentStatus,
    pub transfer_scope: VrpTransferScope,
    pub agreement_json: VrpValidationReport,
    pub remote_handshake_json: Option<VrpFederationHandshake>,
    pub active: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Request payload for cross-server identity attestation.
#[derive(Debug, Serialize, Deserialize)]
pub struct AttestationRequest {
    /// The base URL of the server attesting the identity.
    pub originating_server: String,
    /// The VRP topic used for pseudonym derivation.
    pub topic: String,
    /// The identity commitment (hex).
    pub commitment: String,
    /// The Groth16 proof (JSON object).
    pub proof: serde_json::Value,
    /// The type of participant (e.g., "HUMAN", "AI_AGENT").
    pub participant_type: String,
    /// The signature of the request (hex).
    /// Signed message: SHA256(topic || commitment || participant_type).
    pub signature: String,
}

/// A message relayed from a federation peer.
#[derive(Debug, Serialize, Deserialize)]
pub struct FederatedMessageEnvelope {
    /// Unique public ID of the message (on the originating server).
    pub message_id: String,
    /// The public channel ID.
    pub channel_id: String,
    /// The message content.
    pub content: String,
    /// The sender's pseudonym on the originating server.
    pub sender_pseudonym: String,
    /// The base URL of the originating server.
    pub originating_server: String,
    /// VRP attestation reference (format: "topic:commitment_hex").
    pub attestation_ref: String,
    /// Signature of SHA256(message_id + channel_id + content + sender + originating_server + attestation_ref + created_at).
    pub signature: String,
    /// Creation timestamp (ISO 8601).
    pub created_at: String,
}

/// An RTX bundle relayed from a federation peer.
///
/// When a bundle is published on one server and relayed to a federated peer,
/// it is wrapped in this envelope. The envelope carries:
/// - The original bundle (with transfer scope already applied by the sending server)
/// - The provenance chain tracking all relay hops
/// - The relaying server's Ed25519 signature proving authenticity
///
/// The receiving server validates the signature against the relaying server's
/// public key, checks the federation agreement's transfer scope, and delivers
/// the bundle to local subscribers with `accept_federated = true`.
#[derive(Debug, Serialize, Deserialize)]
pub struct FederatedRtxEnvelope {
    /// The RTX bundle being relayed.
    pub bundle: ReflectionSummaryBundle,
    /// The provenance chain tracking relay hops from origin to this server.
    pub provenance: BundleProvenance,
    /// The base URL of the server sending this relay (the immediate sender).
    pub relaying_server: String,
    /// Ed25519 signature of the relay payload (hex-encoded).
    ///
    /// Signed payload: `bundle_id + relaying_server + origin_server + relay_path_joined`.
    /// The relay path is joined with `|` separators for deterministic signing.
    pub signature: String,
}
