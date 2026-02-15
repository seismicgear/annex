use crate::types::VrpValidationReport;
use rusqlite::{params, Connection};
use thiserror::Error;

/// Errors that can occur during reputation operations.
#[derive(Error, Debug)]
pub enum ReputationError {
    /// A database error occurred.
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    /// A serialization error occurred.
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Records the outcome of a VRP handshake in the log.
pub fn record_vrp_outcome(
    conn: &Connection,
    server_id: i64,
    peer_pseudonym: &str,
    peer_type: &str,
    report: &VrpValidationReport,
) -> Result<(), ReputationError> {
    let report_json = serde_json::to_string(report)?;
    // Use the Display implementation for the status string
    let status_str = report.alignment_status.to_string();

    conn.execute(
        "INSERT INTO vrp_handshake_log (server_id, peer_pseudonym, peer_type, alignment_status, report_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![server_id, peer_pseudonym, peer_type, status_str, report_json],
    )?;
    Ok(())
}

/// Computes the longitudinal reputation score for a peer based on handshake history.
///
/// The score starts at 0.5 (neutral) and evolves based on historical outcomes:
/// - `ALIGNED` outcomes increase the score towards 1.0.
/// - `PARTIAL` outcomes slightly decrease the score.
/// - `CONFLICT` outcomes significantly decrease the score.
///
/// The calculation processes history from oldest to newest to capture trends.
pub fn check_reputation_score(
    conn: &Connection,
    server_id: i64,
    peer_pseudonym: &str,
) -> Result<f32, ReputationError> {
    // Fetch history, ordered by time (oldest first)
    let mut stmt = conn.prepare(
        "SELECT alignment_status FROM vrp_handshake_log
         WHERE server_id = ?1 AND peer_pseudonym = ?2
         ORDER BY created_at ASC",
    )?;

    let rows = stmt.query_map(params![server_id, peer_pseudonym], |row| {
        let status: String = row.get(0)?;
        Ok(status)
    })?;

    let mut score = 0.5; // Start neutral

    for row in rows {
        let status_str = row?;
        match status_str.as_str() {
            "ALIGNED" => {
                // Boost: move 10% of the remaining distance to 1.0
                score += 0.1 * (1.0 - score);
            }
            "PARTIAL" => {
                // Slight penalty: degrade by 5%
                score -= 0.05 * score;
            }
            "CONFLICT" => {
                // Heavy penalty: degrade by 20%
                score -= 0.2 * score;
            }
            _ => {
                // Ignore unknown statuses
            }
        }
    }

    // Ensure bounds (though logic shouldn't exceed them)
    if score > 1.0 {
        score = 1.0;
    }
    if score < 0.0 {
        score = 0.0;
    }

    Ok(score)
}
