//! Nullifier tracking for ZK proofs.
//!
//! Prevents double-spending of identities (double-join) by tracking nullifiers.
//! A nullifier is derived from the identity commitment and the topic.
//! `nullifierHex = sha256(commitmentHex + ":" + topic)`

use crate::IdentityError;
use rusqlite::{Connection, ErrorCode};

/// Checks if a nullifier has already been used for a given topic.
///
/// # Errors
///
/// Returns [`IdentityError::DatabaseError`] if the query fails.
pub fn check_nullifier_exists(
    conn: &Connection,
    topic: &str,
    nullifier_hex: &str,
) -> Result<bool, IdentityError> {
    let count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM zk_nullifiers WHERE topic = ?1 AND nullifier_hex = ?2",
            [topic, nullifier_hex],
            |row| row.get(0),
        )
        .map_err(IdentityError::DatabaseError)?;

    Ok(count > 0)
}

/// Inserts a nullifier into the database.
///
/// `pseudonym_id` and `commitment_hex` are optional denormalized lookup columns
/// added by migration 024. When provided, they enable O(1) pseudonym-to-commitment
/// resolution in `find_commitment_for_pseudonym` instead of an O(N*M) full-table scan.
///
/// # Errors
///
/// Returns [`IdentityError::DuplicateNullifier`] if the nullifier already exists for the topic.
/// Returns [`IdentityError::DatabaseError`] for other database errors.
pub fn insert_nullifier(
    conn: &Connection,
    topic: &str,
    nullifier_hex: &str,
    pseudonym_id: Option<&str>,
    commitment_hex: Option<&str>,
) -> Result<(), IdentityError> {
    let res = conn.execute(
        "INSERT INTO zk_nullifiers (topic, nullifier_hex, pseudonym_id, commitment_hex) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![topic, nullifier_hex, pseudonym_id, commitment_hex],
    );

    match res {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(err, _))
            if err.code == ErrorCode::ConstraintViolation =>
        {
            Err(IdentityError::DuplicateNullifier(topic.to_string()))
        }
        Err(e) => Err(IdentityError::DatabaseError(e)),
    }
}
