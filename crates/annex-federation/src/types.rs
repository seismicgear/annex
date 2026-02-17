use serde::{Deserialize, Serialize};
use annex_vrp::{VrpAlignmentStatus, VrpTransferScope, VrpValidationReport};

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
