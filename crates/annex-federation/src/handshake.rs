use crate::db::create_agreement;
use annex_types::ServerPolicy;
use annex_vrp::{
    validate_federation_handshake, ServerPolicyRoot, VrpAlignmentConfig,
    VrpCapabilitySharingContract, VrpFederationHandshake, VrpTransferAcceptanceConfig,
    VrpValidationReport,
};
use rusqlite::Connection;
use thiserror::Error;

/// Errors that can occur during a federation handshake.
#[derive(Debug, Error)]
pub enum HandshakeError {
    #[error("Database error: {0}")]
    DbError(#[from] rusqlite::Error),
    #[error("Unknown remote instance")]
    UnknownRemoteInstance,
}

/// Processes an incoming federation handshake from a peer server.
///
/// This function:
/// 1. Derives the local server's policy root and anchor.
/// 2. Defines the local capability contract and transfer acceptance config.
/// 3. Validates the incoming handshake using `annex-vrp`.
/// 4. Persists the resulting agreement in the database.
pub fn process_incoming_handshake(
    conn: &Connection,
    local_server_id: i64,
    local_policy: &ServerPolicy,
    remote_instance_id: i64,
    handshake: &VrpFederationHandshake,
) -> Result<VrpValidationReport, HandshakeError> {
    // 1. Derive local policy root and anchor
    let local_policy_root = ServerPolicyRoot::from_policy(local_policy);
    let local_anchor = local_policy_root.to_anchor_snapshot();

    // 2. Define local capability contract
    // In a real implementation, this would be more granular based on policy.
    let mut offered_capabilities = Vec::new();
    if local_policy.voice_enabled {
        offered_capabilities.push("voice".to_string());
    }
    if local_policy.federation_enabled {
        offered_capabilities.push("federation".to_string());
    }

    let required_capabilities = local_policy.agent_required_capabilities.clone();

    let local_contract = VrpCapabilitySharingContract {
        required_capabilities,
        offered_capabilities,
    };

    // 3. Define alignment config
    let alignment_config = VrpAlignmentConfig {
        semantic_alignment_required: false, // Not enabled yet
        min_alignment_score: local_policy.agent_min_alignment_score,
    };

    // 4. Define transfer acceptance config
    // For now, we allow reflection summaries if federation is enabled.
    let transfer_config = VrpTransferAcceptanceConfig {
        allow_reflection_summaries: local_policy.federation_enabled,
        allow_full_knowledge: false, // Conservative default
    };

    // 5. Validate handshake
    let report = validate_federation_handshake(
        &local_anchor,
        &local_contract,
        handshake,
        &alignment_config,
        &transfer_config,
    );

    // 6. Persist agreement
    create_agreement(
        conn,
        local_server_id,
        remote_instance_id,
        &report,
        Some(handshake),
    )?;

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use annex_vrp::{VrpAnchorSnapshot, VrpCapabilitySharingContract};

    #[test]
    fn test_process_handshake() {
        let conn = Connection::open_in_memory().unwrap();
        // Manually create table since migrations are in annex-db and we are in annex-federation test
        conn.execute(
            "CREATE TABLE federation_agreements (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                local_server_id INTEGER NOT NULL,
                remote_instance_id INTEGER NOT NULL,
                alignment_status TEXT NOT NULL,
                transfer_scope TEXT NOT NULL,
                agreement_json TEXT NOT NULL,
                remote_handshake_json TEXT,
                active INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            )",
            [],
        )
        .unwrap();

        let policy = ServerPolicy::default();
        let anchor = VrpAnchorSnapshot::new(&[], &[]);
        let contract = VrpCapabilitySharingContract {
            required_capabilities: vec![],
            offered_capabilities: vec![],
        };
        let handshake = VrpFederationHandshake {
            anchor_snapshot: anchor,
            capability_contract: contract,
        };

        let report = process_incoming_handshake(&conn, 1, &policy, 10, &handshake).unwrap();
        assert_eq!(
            report.alignment_status,
            annex_vrp::VrpAlignmentStatus::Aligned
        );

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM federation_agreements", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    }
}
