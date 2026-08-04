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

/// The identity that already owns a nullifier for a topic.
///
/// Returned by [`existing_nullifier_owner`] so callers can distinguish a
/// legitimate re-authentication (same commitment presenting the same
/// nullifier again) from a genuine conflict (a different commitment
/// colliding on one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NullifierOwner {
    /// Pseudonym derived when the nullifier was first consumed.
    pub pseudonym_id: Option<String>,
    /// Commitment that consumed it. `None` for rows written before
    /// migration 024 added the denormalised column.
    pub commitment_hex: Option<String>,
}

/// Looks up who already consumed a nullifier for a topic.
///
/// Returns `None` when the nullifier is unused.
///
/// # Errors
///
/// Returns [`IdentityError::DatabaseError`] if the query fails.
pub fn existing_nullifier_owner(
    conn: &Connection,
    topic: &str,
    nullifier_hex: &str,
) -> Result<Option<NullifierOwner>, IdentityError> {
    use rusqlite::OptionalExtension;

    conn.query_row(
        "SELECT pseudonym_id, commitment_hex FROM zk_nullifiers \
         WHERE topic = ?1 AND nullifier_hex = ?2",
        [topic, nullifier_hex],
        |row| {
            Ok(NullifierOwner {
                pseudonym_id: row.get(0)?,
                commitment_hex: row.get(1)?,
            })
        },
    )
    .optional()
    .map_err(IdentityError::DatabaseError)
}

/// Backfills the denormalised lookup columns on a pre-migration-024 row.
///
/// Only fills columns that are currently `NULL`, so an existing binding is
/// never overwritten — that binding is what
/// [`existing_nullifier_owner`] relies on to tell re-authentication apart
/// from a conflict.
///
/// # Errors
///
/// Returns [`IdentityError::DatabaseError`] if the update fails.
pub fn backfill_nullifier_owner(
    conn: &Connection,
    topic: &str,
    nullifier_hex: &str,
    pseudonym_id: &str,
    commitment_hex: &str,
) -> Result<(), IdentityError> {
    conn.execute(
        "UPDATE zk_nullifiers \
            SET pseudonym_id = COALESCE(pseudonym_id, ?3), \
                commitment_hex = COALESCE(commitment_hex, ?4) \
          WHERE topic = ?1 AND nullifier_hex = ?2",
        rusqlite::params![topic, nullifier_hex, pseudonym_id, commitment_hex],
    )
    .map_err(IdentityError::DatabaseError)?;
    Ok(())
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
