use crate::types::FederationAgreement;
use annex_vrp::{VrpFederationHandshake, VrpValidationReport};
use rusqlite::{params, Connection, Result};

/// Creates a new federation agreement record.
pub fn create_agreement(
    conn: &mut Connection,
    local_server_id: i64,
    remote_instance_id: i64,
    report: &VrpValidationReport,
    handshake: Option<&VrpFederationHandshake>,
) -> Result<i64> {
    let report_json = serde_json::to_string(report).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            5, // index of agreement_json
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;

    let handshake_json = if let Some(h) = handshake {
        Some(serde_json::to_string(h).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(
                6, // index of remote_handshake_json
                rusqlite::types::Type::Text,
                Box::new(e),
            )
        })?)
    } else {
        None
    };

    // Store status and scope as string representations for queryability
    let alignment_status = report.alignment_status.to_string();
    let transfer_scope = report.transfer_scope.to_string();

    // Use a savepoint to ensure atomicity of deactivation + insertion.
    // If this is called from within an existing transaction, the savepoint
    // acts as a nested transaction. If not, rusqlite's savepoint starts
    // an implicit outer transaction. A crash between deactivation and
    // insertion previously could leave the federation link permanently broken.
    let sp = conn.savepoint()?;

    // Deactivate any existing active agreements for this instance,
    // scoped to local_server_id to prevent multi-tenant interference.
    sp.execute(
        "UPDATE federation_agreements SET active = 0, updated_at = datetime('now')
         WHERE local_server_id = ?1 AND remote_instance_id = ?2 AND active = 1",
        params![local_server_id, remote_instance_id],
    )?;

    sp.execute(
        "INSERT INTO federation_agreements (
            local_server_id,
            remote_instance_id,
            alignment_status,
            transfer_scope,
            agreement_json,
            remote_handshake_json
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            local_server_id,
            remote_instance_id,
            alignment_status,
            transfer_scope,
            report_json,
            handshake_json
        ],
    )?;

    let id = sp.last_insert_rowid();
    sp.commit()?;

    Ok(id)
}

/// Retrieves the active federation agreement for a remote instance.
pub fn get_agreement(
    conn: &Connection,
    remote_instance_id: i64,
) -> Result<Option<FederationAgreement>> {
    let mut stmt = conn.prepare(
        "SELECT id, local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, remote_handshake_json, active, created_at, updated_at
         FROM federation_agreements
         WHERE remote_instance_id = ?1 AND active = 1",
    )?;

    let mut rows = stmt.query(params![remote_instance_id])?;

    if let Some(row) = rows.next()? {
        let agreement_json_str: String = row.get(5)?;
        let agreement_json: VrpValidationReport = serde_json::from_str(&agreement_json_str)
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    5,
                    rusqlite::types::Type::Text,
                    Box::new(e),
                )
            })?;

        let handshake_json_str: Option<String> = row.get(6)?;
        let remote_handshake_json: Option<VrpFederationHandshake> =
            if let Some(s) = handshake_json_str {
                Some(serde_json::from_str(&s).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?)
            } else {
                None
            };

        // We use the values from the deserialized report to ensure consistency
        Ok(Some(FederationAgreement {
            id: row.get(0)?,
            local_server_id: row.get(1)?,
            remote_instance_id: row.get(2)?,
            alignment_status: agreement_json.alignment_status,
            transfer_scope: agreement_json.transfer_scope,
            agreement_json,
            remote_handshake_json,
            active: row.get(7)?,
            created_at: row.get(8)?,
            updated_at: row.get(9)?,
        }))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use annex_vrp::{VrpAlignmentStatus, VrpTransferScope, VrpValidationReport};

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
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
        conn
    }

    fn make_report() -> VrpValidationReport {
        VrpValidationReport {
            alignment_status: VrpAlignmentStatus::Aligned,
            transfer_scope: VrpTransferScope::ReflectionSummariesOnly,
            alignment_score: 1.0,
            negotiation_notes: vec![],
        }
    }

    #[test]
    fn create_agreement_deactivates_old_and_inserts_new_atomically() {
        let mut conn = setup_db();
        let report = make_report();

        // Create first agreement
        let id1 = create_agreement(&mut conn, 1, 10, &report, None).unwrap();
        assert!(id1 > 0);

        // Verify it's active
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM federation_agreements WHERE remote_instance_id = 10 AND active = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        // Create second agreement for same remote instance
        let id2 = create_agreement(&mut conn, 1, 10, &report, None).unwrap();
        assert!(id2 > id1);

        // Verify old is deactivated and new is active
        let active_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM federation_agreements WHERE remote_instance_id = 10 AND active = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active_count, 1, "exactly one agreement should be active");

        let active_id: i64 = conn
            .query_row(
                "SELECT id FROM federation_agreements WHERE remote_instance_id = 10 AND active = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(active_id, id2, "the newest agreement should be active");
    }

    #[test]
    fn create_agreement_scoped_to_local_server_id() {
        let mut conn = setup_db();
        let report = make_report();

        // Server 1 creates agreement with remote 10
        create_agreement(&mut conn, 1, 10, &report, None).unwrap();
        // Server 2 creates agreement with same remote 10
        create_agreement(&mut conn, 2, 10, &report, None).unwrap();

        // Both should be active (different local servers)
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM federation_agreements WHERE remote_instance_id = 10 AND active = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 2, "agreements from different servers should coexist");
    }
}
