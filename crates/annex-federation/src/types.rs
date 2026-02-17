use annex_vrp::{VrpAlignmentStatus, VrpTransferScope, VrpValidationReport};
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

/// Message envelope for cross-server message relay.
#[derive(Debug, Serialize, Deserialize)]
pub struct FederatedMessageEnvelope {
    /// Unique message ID (UUID).
    pub message_id: String,
    /// Channel ID where the message was sent.
    pub channel_id: String,
    /// Message content.
    pub content: String,
    /// Pseudonym of the sender.
    pub sender_pseudonym: String,
    /// Base URL of the originating server.
    pub originating_server: String,
    /// VRP attestation reference (format: "topic:commitment_hex").
    pub attestation_ref: String,
    /// Signature (hex) of SHA256(message_id + channel_id + content + sender + originating_server + attestation_ref + created_at).
    pub signature: String,
    /// Creation timestamp (ISO 8601).
    pub created_at: String,
}
