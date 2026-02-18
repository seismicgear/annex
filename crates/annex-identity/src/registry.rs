//! Identity Registry.
//!
//! Handles high-level identity registration: inserting into `vrp_identities`
//! and updating the Merkle tree atomically.

use crate::{IdentityError, MerkleTree, RoleCode};
use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

/// VRP Topic definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VrpTopic {
    /// The unique topic identifier (e.g., "annex:server:v1").
    pub topic: String,
    /// Human-readable description.
    pub description: String,
}

/// VRP Role definition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VrpRoleEntry {
    /// The numeric role code.
    pub role_code: u8,
    /// The string label (e.g., "HUMAN").
    pub label: String,
}

/// Result of a successful registration.
#[derive(Debug)]
pub struct RegistrationResult {
    /// The unique ID in the `vrp_identities` table (not the leaf index).
    pub identity_id: i64,
    /// The Merkle tree leaf index assigned to this identity.
    pub leaf_index: usize,
    /// The updated Merkle root (hex string).
    pub root_hex: String,
    /// Merkle path elements (hex strings) for the proof.
    pub path_elements: Vec<String>,
    /// Merkle path indices (0 or 1).
    pub path_indices: Vec<u8>,
}

/// Registers a new identity commitment.
///
/// 1. Checks if the commitment is already registered in `vrp_identities`.
/// 2. Inserts the new identity into `vrp_identities`.
/// 3. Inserts the commitment into the Merkle tree.
/// 4. Persists the tree update to `vrp_leaves` and `vrp_roots`.
/// 5. Returns the Merkle path and new root.
///
/// All database operations are wrapped in a transaction.
///
/// # Errors
///
/// Returns [`IdentityError::InvalidCommitmentFormat`] if commitment is invalid hex.
/// Returns [`IdentityError::DuplicateNullifier`] (reused error variant) if commitment already exists.
/// Returns [`IdentityError::TreeFull`] if the tree is full.
/// Returns [`IdentityError::DatabaseError`] if SQL fails.
pub fn register_identity(
    tree: &mut MerkleTree,
    conn: &mut Connection,
    commitment_hex: &str,
    role: RoleCode,
    node_id: i64,
) -> Result<RegistrationResult, IdentityError> {
    // Validate format
    if commitment_hex.len() != 64 || !commitment_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(IdentityError::InvalidCommitmentFormat);
    }

    // Normalize to lowercase to prevent case-mismatch bugs when deriving nullifiers.
    // derive_nullifier_hex() requires lowercase hex; storing uppercase would cause
    // verification failures when the stored commitment is used for nullifier derivation.
    let commitment_hex = &commitment_hex.to_ascii_lowercase();

    // Convert commitment to Fr leaf
    let leaf_bytes = hex::decode(commitment_hex).map_err(|_| IdentityError::InvalidHex)?;
    let leaf = Fr::from_be_bytes_mod_order(&leaf_bytes);

    // 1. Preview Merkle Tree changes (Read-Only)
    // This calculates the new root and updates without modifying the tree.
    let (leaf_index, new_root, updates) = tree.preview_insert(leaf)?;

    // Start transaction
    let tx = conn.transaction().map_err(IdentityError::DatabaseError)?;

    // 2. Check & Insert into vrp_identities
    // We try to insert directly. If it fails due to UNIQUE constraint, it's a duplicate.
    let identity_id = match tx.execute(
        "INSERT INTO vrp_identities (commitment_hex, role_code, node_id) VALUES (?1, ?2, ?3)",
        params![commitment_hex, role.as_u8(), node_id],
    ) {
        Ok(_) => tx.last_insert_rowid(),
        Err(rusqlite::Error::SqliteFailure(err, _)) => {
            if err.code == rusqlite::ErrorCode::ConstraintViolation {
                // Determine if it was commitment constraint
                return Err(IdentityError::DuplicateNullifier(format!(
                    "commitment '{}' already registered",
                    commitment_hex
                )));
            }
            return Err(IdentityError::DatabaseError(
                rusqlite::Error::SqliteFailure(err, None),
            ));
        }
        Err(e) => return Err(IdentityError::DatabaseError(e)),
    };

    // 3. Persist Merkle Tree update (In Transaction)
    tree.persist_leaf_and_root(&tx, leaf_index, leaf, new_root)?;

    // 4. Commit Transaction
    tx.commit().map_err(IdentityError::DatabaseError)?;

    // 5. Apply updates to In-Memory Tree
    // Only done if transaction succeeds.
    tree.apply_updates(leaf_index + 1, updates);

    // 5. Generate Proof (Read-only)
    let (path_elements_fr, path_indices) = tree.get_proof(leaf_index)?;

    let path_elements = path_elements_fr
        .into_iter()
        .map(|fr| hex::encode(fr.into_bigint().to_bytes_be()))
        .collect();

    let root_hex = hex::encode(new_root.into_bigint().to_bytes_be());

    Ok(RegistrationResult {
        identity_id,
        leaf_index,
        root_hex,
        path_elements,
        path_indices,
    })
}

/// Retrieves the Merkle path for an existing commitment.
///
/// 1. Lookups the leaf index in `vrp_leaves` using the commitment hex.
/// 2. Calls `tree.get_proof(leaf_index)` to generate the path.
///
/// # Errors
///
/// Returns [`IdentityError::CommitmentNotFound`] if the commitment does not exist.
/// Returns [`IdentityError::DatabaseError`] if SQL fails.
pub fn get_path_for_commitment(
    tree: &MerkleTree,
    conn: &Connection,
    commitment_hex: &str,
) -> Result<(usize, String, Vec<String>, Vec<u8>), IdentityError> {
    let leaf_index: Option<usize> = conn
        .query_row(
            "SELECT leaf_index FROM vrp_leaves WHERE commitment_hex = ?1",
            params![commitment_hex],
            |row| row.get(0),
        )
        .optional()
        .map_err(IdentityError::DatabaseError)?;

    let leaf_index = match leaf_index {
        Some(idx) => idx,
        None => {
            return Err(IdentityError::CommitmentNotFound(
                commitment_hex.to_string(),
            ))
        }
    };

    let (path_elements_fr, path_indices) = tree.get_proof(leaf_index)?;

    let path_elements = path_elements_fr
        .into_iter()
        .map(|fr| hex::encode(fr.into_bigint().to_bytes_be()))
        .collect();

    let root_hex = hex::encode(tree.root().into_bigint().to_bytes_be());

    Ok((leaf_index, root_hex, path_elements, path_indices))
}

/// Retrieves all registered VRP topics.
pub fn get_all_topics(conn: &Connection) -> Result<Vec<VrpTopic>, IdentityError> {
    let mut stmt = conn
        .prepare("SELECT topic, description FROM vrp_topics ORDER BY created_at ASC")
        .map_err(IdentityError::DatabaseError)?;

    let topics = stmt
        .query_map([], |row| {
            Ok(VrpTopic {
                topic: row.get(0)?,
                description: row.get(1)?,
            })
        })
        .map_err(IdentityError::DatabaseError)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(IdentityError::DatabaseError)?;

    Ok(topics)
}

/// Retrieves all registered VRP roles.
pub fn get_all_roles(conn: &Connection) -> Result<Vec<VrpRoleEntry>, IdentityError> {
    let mut stmt = conn
        .prepare("SELECT role_code, label FROM vrp_roles ORDER BY role_code ASC")
        .map_err(IdentityError::DatabaseError)?;

    let roles = stmt
        .query_map([], |row| {
            Ok(VrpRoleEntry {
                role_code: row.get(0)?,
                label: row.get(1)?,
            })
        })
        .map_err(IdentityError::DatabaseError)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(IdentityError::DatabaseError)?;

    Ok(roles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use annex_db::run_migrations;

    #[test]
    fn test_register_identity_success() {
        let mut conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let mut tree = MerkleTree::new(5).unwrap();
        let commitment = "0000000000000000000000000000000000000000000000000000000000000001";

        let result = register_identity(&mut tree, &mut conn, commitment, RoleCode::Human, 100)
            .expect("registration should succeed");

        assert_eq!(result.leaf_index, 0);
        assert_eq!(result.path_indices.len(), 5);
        assert_eq!(result.path_indices, vec![0, 0, 0, 0, 0]); // First leaf path is all left

        // Verify it's in DB
        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM vrp_identities WHERE commitment_hex = ?1)",
                params![commitment],
                |row| row.get(0),
            )
            .unwrap();
        assert!(exists);
    }

    #[test]
    fn test_register_duplicate_commitment_fails() {
        let mut conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let mut tree = MerkleTree::new(5).unwrap();
        let commitment = "0000000000000000000000000000000000000000000000000000000000000001";

        register_identity(&mut tree, &mut conn, commitment, RoleCode::Human, 100).unwrap();

        let err = register_identity(
            &mut tree,
            &mut conn,
            commitment,
            RoleCode::AiAgent, // Even with different role
            101,
        )
        .unwrap_err();

        match err {
            IdentityError::DuplicateNullifier(msg) => {
                assert!(msg.contains("already registered"));
            }
            _ => panic!("expected DuplicateNullifier error, got {:?}", err),
        }
    }

    #[test]
    fn test_register_identity_normalizes_to_lowercase() {
        let mut conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let mut tree = MerkleTree::new(5).unwrap();
        // Use uppercase hex (valid ascii_hexdigit but not lowercase)
        let commitment_upper = "000000000000000000000000000000000000000000000000000000000000ABCD";

        let result = register_identity(&mut tree, &mut conn, commitment_upper, RoleCode::Human, 100)
            .expect("registration with uppercase should succeed");

        // Verify the stored commitment is lowercase
        let stored: String = conn
            .query_row(
                "SELECT commitment_hex FROM vrp_identities WHERE commitment_hex = ?1",
                params![commitment_upper.to_ascii_lowercase()],
                |row| row.get(0),
            )
            .expect("should find commitment stored as lowercase");

        assert_eq!(
            stored,
            commitment_upper.to_ascii_lowercase(),
            "commitment should be stored as lowercase hex"
        );

        // Verify get_path works with the original uppercase input
        // Since both vrp_leaves and vrp_identities store lowercase now,
        // lookups should use the normalized form.
        let path_result = get_path_for_commitment(
            &tree,
            &conn,
            &commitment_upper.to_ascii_lowercase(),
        );
        assert!(path_result.is_ok(), "path lookup should work with lowercase commitment");
    }

    #[test]
    fn test_register_identity_invalid_format_rejected() {
        let mut conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();

        let mut tree = MerkleTree::new(5).unwrap();

        // Too short
        let err = register_identity(&mut tree, &mut conn, "abcd", RoleCode::Human, 100)
            .unwrap_err();
        assert_eq!(err, IdentityError::InvalidCommitmentFormat);

        // Non-hex characters
        let err = register_identity(
            &mut tree,
            &mut conn,
            "000000000000000000000000000000000000000000000000000000000000ZZZZ",
            RoleCode::Human,
            100,
        )
        .unwrap_err();
        assert_eq!(err, IdentityError::InvalidCommitmentFormat);
    }
}
