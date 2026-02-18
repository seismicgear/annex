//! Nullifier tracking for ZK proofs.
//!
//! Prevents double-spending of identities (double-join) by tracking nullifiers.
//! A nullifier is derived from the identity commitment and the topic.
//! `nullifierHex = sha256(commitmentHex + ":" + topic)`

use crate::IdentityError;
use rusqlite::{params, Connection, ErrorCode, OptionalExtension};

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
/// This is the backwards-compatible entry point that stores (topic, nullifier_hex)
/// without the pseudonym/commitment lookup columns. Use [`insert_nullifier_with_lookup`]
/// when pseudonym and commitment are available for O(1) reverse lookup.
///
/// # Errors
///
/// Returns [`IdentityError::DuplicateNullifier`] if the nullifier already exists for the topic.
/// Returns [`IdentityError::DatabaseError`] for other database errors.
pub fn insert_nullifier(
    conn: &Connection,
    topic: &str,
    nullifier_hex: &str,
) -> Result<(), IdentityError> {
    insert_nullifier_with_lookup(conn, topic, nullifier_hex, None, None)
}

/// Inserts a nullifier with optional pseudonym and commitment for indexed reverse lookup.
///
/// When `pseudonym_id` and `commitment_hex` are provided, they are stored alongside
/// the nullifier to enable O(1) pseudonym → commitment lookups (via migration 024 index)
/// instead of the O(N*M) brute-force scan that was previously required.
///
/// # Errors
///
/// Returns [`IdentityError::DuplicateNullifier`] if the nullifier already exists for the topic.
/// Returns [`IdentityError::DatabaseError`] for other database errors.
pub fn insert_nullifier_with_lookup(
    conn: &Connection,
    topic: &str,
    nullifier_hex: &str,
    pseudonym_id: Option<&str>,
    commitment_hex: Option<&str>,
) -> Result<(), IdentityError> {
    let res = conn.execute(
        "INSERT INTO zk_nullifiers (topic, nullifier_hex, pseudonym_id, commitment_hex) VALUES (?1, ?2, ?3, ?4)",
        params![topic, nullifier_hex, pseudonym_id, commitment_hex],
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

/// Looks up the commitment hex and topic for a given pseudonym using the indexed column.
///
/// Returns `Some((commitment_hex, topic))` if found, `None` if no indexed record exists
/// (e.g., for rows created before migration 024).
///
/// # Errors
///
/// Returns [`IdentityError::DatabaseError`] if the query fails.
pub fn lookup_commitment_by_pseudonym(
    conn: &Connection,
    pseudonym_id: &str,
) -> Result<Option<(String, String)>, IdentityError> {
    conn.query_row(
        "SELECT commitment_hex, topic FROM zk_nullifiers WHERE pseudonym_id = ?1 LIMIT 1",
        params![pseudonym_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(IdentityError::DatabaseError)
}
