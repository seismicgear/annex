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

/// Retrieves the active federation agreement for a remote instance,
/// scoped to the local server to prevent cross-tenant leakage.
pub fn get_agreement(
    conn: &Connection,
    local_server_id: i64,
    remote_instance_id: i64,
) -> Result<Option<FederationAgreement>> {
    let mut stmt = conn.prepare(
        "SELECT id, local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, remote_handshake_json, active, created_at, updated_at
         FROM federation_agreements
         WHERE local_server_id = ?1 AND remote_instance_id = ?2 AND active = 1",
    )?;

    let mut rows = stmt.query(params![local_server_id, remote_instance_id])?;

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

/// Revokes a federation agreement by setting active=0.
pub fn revoke_agreement(
    conn: &Connection,
    agreement_id: i64,
    local_server_id: i64,
) -> Result<bool> {
    let rows = conn.execute(
        "UPDATE federation_agreements SET active = 0, updated_at = datetime('now')
         WHERE id = ?1 AND local_server_id = ?2 AND active = 1",
        params![agreement_id, local_server_id],
    )?;
    Ok(rows > 0)
}

/// Lists all active federation agreements for a server.
pub fn list_active_agreements(
    conn: &Connection,
    local_server_id: i64,
) -> Result<Vec<(i64, i64, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT fa.id, fa.remote_instance_id, fa.alignment_status, fa.transfer_scope
         FROM federation_agreements fa
         WHERE fa.local_server_id = ?1 AND fa.active = 1
         ORDER BY fa.created_at DESC",
    )?;
    let rows = stmt.query_map(params![local_server_id], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })?;
    rows.collect()
}

/// Records that an agreement is still carrying traffic.
///
/// `expire_stale_agreements` reaps agreements whose `updated_at` has not
/// moved within the TTL, and its documentation said that column "is
/// refreshed on every re-handshake / policy re-evaluation, so this only
/// reaps peers that have gone silent". Neither half held for a working
/// link. There is no periodic re-handshake — the only outbound one fires
/// on a local policy change — and `recalculate_federation_agreements` skips
/// the UPDATE entirely when the verdict is unchanged, which is exactly the
/// case for a healthy peer. Ordinary federated traffic only ever read the
/// row. So `updated_at` stayed frozen at the moment of first contact and
/// every federation link deactivated itself `agreement_ttl_days` later,
/// while messages were still flowing.
///
/// This is the liveness signal that was missing. The guard on the age keeps
/// it to at most one write per peer per day rather than one per delivered
/// message, so a busy link does not turn every inbound envelope into an
/// extra UPDATE.
pub fn touch_agreement(
    conn: &Connection,
    local_server_id: i64,
    remote_instance_id: i64,
) -> Result<usize> {
    let rows = conn.execute(
        "UPDATE federation_agreements SET updated_at = datetime('now')
         WHERE local_server_id = ?1 AND remote_instance_id = ?2 AND active = 1
         AND julianday('now') - julianday(updated_at) > 1.0",
        params![local_server_id, remote_instance_id],
    )?;
    Ok(rows)
}

/// Expires agreements older than the given number of days.
pub fn expire_stale_agreements(
    conn: &Connection,
    local_server_id: i64,
    max_age_days: u32,
) -> Result<usize> {
    let rows = conn.execute(
        "UPDATE federation_agreements SET active = 0, updated_at = datetime('now')
         WHERE local_server_id = ?1 AND active = 1
         AND julianday('now') - julianday(updated_at) > ?2",
        params![local_server_id, max_age_days],
    )?;
    Ok(rows)
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

#[cfg(test)]
mod liveness_tests {
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

    fn report() -> VrpValidationReport {
        VrpValidationReport {
            alignment_status: VrpAlignmentStatus::Aligned,
            transfer_scope: VrpTransferScope::ReflectionSummariesOnly,
            alignment_score: 1.0,
            negotiation_notes: vec![],
        }
    }

    fn backdate(conn: &Connection, days: i64) {
        conn.execute(
            &format!(
                "UPDATE federation_agreements SET updated_at = datetime('now', '-{days} days')"
            ),
            [],
        )
        .unwrap();
    }

    /// A link carrying traffic must not be reaped.
    ///
    /// `expire_stale_agreements` reads `updated_at`, and nothing on the
    /// message path used to write it — no periodic re-handshake exists, and
    /// `recalculate_federation_agreements` skips the UPDATE when the verdict
    /// is unchanged, which is the healthy case. So a perfectly good
    /// federation relationship deactivated itself `agreement_ttl_days` after
    /// first contact, while messages were still flowing. The touch is what
    /// makes the expiry task mean "gone silent" rather than "existed for a
    /// month".
    #[test]
    fn touching_an_agreement_saves_it_from_expiry() {
        let mut conn = setup_db();
        create_agreement(&mut conn, 1, 10, &report(), None).unwrap();
        backdate(&conn, 45);

        let touched = touch_agreement(&conn, 1, 10).expect("touch");
        assert_eq!(touched, 1, "the touch did not update the row");

        let expired = expire_stale_agreements(&conn, 1, 30).expect("expire");
        assert_eq!(expired, 0, "an actively-used agreement was expired");
    }

    /// The behaviour the task exists for has to survive the fix.
    #[test]
    fn a_silent_peer_is_still_expired() {
        let mut conn = setup_db();
        create_agreement(&mut conn, 1, 10, &report(), None).unwrap();
        backdate(&conn, 45);

        let expired = expire_stale_agreements(&conn, 1, 30).expect("expire");
        assert_eq!(expired, 1, "a peer that has not been heard from was kept");
    }

    /// The age guard keeps a busy link from turning every inbound envelope
    /// into an extra UPDATE.
    #[test]
    fn a_recently_touched_agreement_is_not_written_again() {
        let mut conn = setup_db();
        create_agreement(&mut conn, 1, 10, &report(), None).unwrap();

        let touched = touch_agreement(&conn, 1, 10).expect("touch");
        assert_eq!(touched, 0, "a fresh row should not be rewritten");
    }

    /// The touch is scoped to one relationship, not "any agreement".
    #[test]
    fn touching_one_peer_does_not_save_another() {
        let mut conn = setup_db();
        create_agreement(&mut conn, 1, 10, &report(), None).unwrap();
        create_agreement(&mut conn, 1, 11, &report(), None).unwrap();
        backdate(&conn, 45);

        touch_agreement(&conn, 1, 10).expect("touch");

        let expired = expire_stale_agreements(&conn, 1, 30).expect("expire");
        assert_eq!(expired, 1, "only the untouched peer should expire");
    }
}
