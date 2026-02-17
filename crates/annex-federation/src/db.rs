use crate::types::FederationAgreement;
use annex_vrp::VrpValidationReport;
use rusqlite::{params, Connection, Result};

/// Creates a new federation agreement record.
pub fn create_agreement(
    conn: &Connection,
    local_server_id: i64,
    remote_instance_id: i64,
    report: &VrpValidationReport,
) -> Result<i64> {
    let report_json = serde_json::to_string(report).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(
            5, // index of agreement_json
            rusqlite::types::Type::Text,
            Box::new(e),
        )
    })?;

    // Store status and scope as string representations for queryability
    let alignment_status = report.alignment_status.to_string();
    let transfer_scope = report.transfer_scope.to_string();

    conn.execute(
        "INSERT INTO federation_agreements (
            local_server_id,
            remote_instance_id,
            alignment_status,
            transfer_scope,
            agreement_json
        ) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            local_server_id,
            remote_instance_id,
            alignment_status,
            transfer_scope,
            report_json
        ],
    )?;

    Ok(conn.last_insert_rowid())
}

/// Retrieves the active federation agreement for a remote instance.
pub fn get_agreement(
    conn: &Connection,
    remote_instance_id: i64,
) -> Result<Option<FederationAgreement>> {
    let mut stmt = conn.prepare(
        "SELECT id, local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active, created_at, updated_at
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

        // We use the values from the deserialized report to ensure consistency
        Ok(Some(FederationAgreement {
            id: row.get(0)?,
            local_server_id: row.get(1)?,
            remote_instance_id: row.get(2)?,
            alignment_status: agreement_json.alignment_status,
            transfer_scope: agreement_json.transfer_scope,
            agreement_json,
            active: row.get(6)?,
            created_at: row.get(7)?,
            updated_at: row.get(8)?,
        }))
    } else {
        Ok(None)
    }
}
