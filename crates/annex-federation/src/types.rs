use annex_vrp::{VrpAlignmentStatus, VrpTransferScope, VrpValidationReport};
use serde::{Deserialize, Serialize};

/// A persistent federation agreement between two servers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationAgreement {
    /// Unique ID of the agreement.
    pub id: i64,
    /// ID of the local server.
    pub local_server_id: i64,
    /// ID of the remote instance.
    pub remote_instance_id: i64,
    /// Negotiated alignment status.
    pub alignment_status: VrpAlignmentStatus,
    /// Negotiated transfer scope.
    pub transfer_scope: VrpTransferScope,
    /// Full JSON of the agreement (including the report).
    pub agreement_json: VrpValidationReport,
    /// Whether the agreement is currently active.
    pub active: bool,
    /// Creation timestamp (ISO 8601).
    pub created_at: String,
    /// Last update timestamp (ISO 8601).
    pub updated_at: String,
}
