//! Poseidon Merkle Tree implementation.
//!
//! A binary Merkle tree using Poseidon hash function.
//! Supports append-only insertion and proof generation.

use crate::{poseidon::hash_inputs, IdentityError};
use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

/// A Poseidon Merkle tree.
///
/// Stores leaves and internal nodes in a sparse map to support large depths
/// while keeping memory usage proportional to the number of inserted leaves.
#[derive(Debug)]
pub struct MerkleTree {
    /// Depth of the tree (number of levels excluding root).
    pub depth: usize,
    /// Next available leaf index for insertion.
    pub next_index: usize,
    /// Sparse storage for nodes. Key: (level, index).
    /// Level 0 is leaves. Level `depth` is root.
    nodes: HashMap<(usize, usize), Fr>,
    /// Precomputed zero hashes for each level.
    /// zeros[i] is the default value for a node at level i.
    zeros: Vec<Fr>,
}

impl MerkleTree {
    /// Creates a new empty Merkle tree with the given depth.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::PoseidonError`] if zero hash precomputation fails.
    pub fn new(depth: usize) -> Result<Self, IdentityError> {
        let mut zeros = Vec::with_capacity(depth + 1);
        zeros.push(Fr::from(0));
        for i in 0..depth {
            let zero = zeros[i];
            let hash = hash_inputs(&[zero, zero])?;
            zeros.push(hash);
        }

        Ok(Self {
            depth,
            next_index: 0,
            nodes: HashMap::new(),
            zeros,
        })
    }

    /// Inserts a leaf into the next available slot.
    ///
    /// Returns the index of the inserted leaf.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::TreeFull`] if the tree is full.
    /// Returns [`IdentityError::PoseidonError`] if hashing fails.
    pub fn insert(&mut self, leaf: Fr) -> Result<usize, IdentityError> {
        if self.next_index >= 1 << self.depth {
            return Err(IdentityError::TreeFull);
        }

        let index = self.next_index;
        self.next_index += 1;

        // Update path to root
        let mut current_idx = index;
        let mut current_val = leaf;

        // Insert leaf
        self.nodes.insert((0, current_idx), current_val);

        for level in 0..self.depth {
            let sibling_idx = current_idx ^ 1;
            let sibling_val = *self
                .nodes
                .get(&(level, sibling_idx))
                .unwrap_or(&self.zeros[level]);

            let parent_val = if current_idx & 1 == 0 {
                // Current is left, sibling is right
                hash_inputs(&[current_val, sibling_val])?
            } else {
                // Current is right, sibling is left
                hash_inputs(&[sibling_val, current_val])?
            };

            current_idx /= 2;
            current_val = parent_val;
            self.nodes.insert((level + 1, current_idx), current_val);
        }

        Ok(index)
    }

    /// Returns the current Merkle root.
    pub fn root(&self) -> Fr {
        *self
            .nodes
            .get(&(self.depth, 0))
            .unwrap_or(&self.zeros[self.depth])
    }

    /// Generates a Merkle proof for the leaf at the given index.
    ///
    /// Returns a tuple `(path_elements, path_indices)`.
    /// `path_elements`: The sibling hashes along the path to the root.
    /// `path_indices`: The direction bits (0 for left, 1 for right) indicating
    /// where the current node is relative to its sibling.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::InvalidIndex`] if `index` is out of bounds (>= next_index).
    pub fn get_proof(&self, index: usize) -> Result<(Vec<Fr>, Vec<u8>), IdentityError> {
        if index >= self.next_index {
            return Err(IdentityError::InvalidIndex(index));
        }

        let mut path_elements = Vec::with_capacity(self.depth);
        let mut path_indices = Vec::with_capacity(self.depth);

        let mut current_idx = index;

        for level in 0..self.depth {
            let sibling_idx = current_idx ^ 1;
            let sibling_val = *self
                .nodes
                .get(&(level, sibling_idx))
                .unwrap_or(&self.zeros[level]);

            path_elements.push(sibling_val);
            path_indices.push((current_idx % 2) as u8);

            current_idx /= 2;
        }

        Ok((path_elements, path_indices))
    }

    /// Reconstructs the Merkle tree from the database.
    ///
    /// Reads leaves from `vrp_leaves` in leaf_index order and inserts them into a new tree.
    /// Verifies that the final root matches the active root in `vrp_roots` (if any).
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::DatabaseError`] if a database operation fails.
    /// Returns [`IdentityError::InvalidHex`] if a stored commitment is invalid hex.
    /// Returns [`IdentityError::PoseidonError`] if hashing fails during reconstruction.
    pub fn restore(conn: &Connection, depth: usize) -> Result<Self, IdentityError> {
        let mut tree = Self::new(depth)?;

        let mut stmt = conn
            .prepare("SELECT commitment_hex FROM vrp_leaves ORDER BY leaf_index ASC")
            .map_err(|e| IdentityError::DatabaseError(e.to_string()))?;

        let leaf_iter = stmt
            .query_map([], |row| {
                let hex_str: String = row.get(0)?;
                Ok(hex_str)
            })
            .map_err(|e| IdentityError::DatabaseError(e.to_string()))?;

        for leaf_res in leaf_iter {
            let leaf_hex = leaf_res.map_err(|e| IdentityError::DatabaseError(e.to_string()))?;
            let leaf_bytes = hex::decode(&leaf_hex).map_err(|_| IdentityError::InvalidHex)?;
            let leaf = Fr::from_be_bytes_mod_order(&leaf_bytes);
            tree.insert(leaf)?;
        }

        // Verify root
        let current_root = tree.root();
        let current_root_bytes = current_root.into_bigint().to_bytes_be();
        let current_root_hex = hex::encode(current_root_bytes);

        // Check if there is an active root in DB
        let stored_root: Option<String> = conn
            .query_row(
                "SELECT root_hex FROM vrp_roots WHERE active = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| IdentityError::DatabaseError(e.to_string()))?;

        if let Some(stored_hex) = stored_root {
            if stored_hex != current_root_hex {
                tracing::warn!(
                    "Merkle root mismatch: computed {}, stored active {}. Proceeding with computed root.",
                    current_root_hex,
                    stored_hex
                );
            }
        }

        Ok(tree)
    }

    /// Persists a leaf insertion into the database.
    ///
    /// This should be called immediately after `insert`.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::DatabaseError`] if a database operation fails.
    pub fn persist_leaf(
        &self,
        conn: &Connection,
        index: usize,
        leaf: Fr,
    ) -> Result<(), IdentityError> {
        let leaf_bytes = leaf.into_bigint().to_bytes_be();
        let leaf_hex = hex::encode(leaf_bytes);

        conn.execute(
            "INSERT INTO vrp_leaves (leaf_index, commitment_hex) VALUES (?1, ?2)",
            params![index, leaf_hex],
        )
        .map_err(|e| IdentityError::DatabaseError(e.to_string()))?;

        Ok(())
    }

    /// Persists the current root as the active root in the database.
    ///
    /// Sets the new root as active and deactivates any previous active root.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::DatabaseError`] if a database operation fails.
    pub fn persist_root(&self, conn: &Connection) -> Result<(), IdentityError> {
        let root = self.root();
        let root_bytes = root.into_bigint().to_bytes_be();
        let root_hex = hex::encode(root_bytes);

        // Transaction to ensure atomicity
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| IdentityError::DatabaseError(e.to_string()))?;

        // Deactivate all existing roots
        tx.execute("UPDATE vrp_roots SET active = 0 WHERE active = 1", [])
            .map_err(|e| IdentityError::DatabaseError(e.to_string()))?;

        // Insert new active root (or update if exists)
        tx.execute(
            "INSERT INTO vrp_roots (root_hex, active) VALUES (?1, 1)
             ON CONFLICT(root_hex) DO UPDATE SET active = 1",
            params![root_hex],
        )
        .map_err(|e| IdentityError::DatabaseError(e.to_string()))?;

        tx.commit()
            .map_err(|e| IdentityError::DatabaseError(e.to_string()))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_merkle_tree_init() {
        let tree = MerkleTree::new(5).expect("failed to create tree");
        assert_eq!(tree.depth, 5);
        assert_eq!(tree.next_index, 0);
        // Root should be zero hash
        // zeros[0] = 0
        // zeros[1] = hash(0,0)
        // ...
        assert_eq!(tree.root(), tree.zeros[5]);
    }

    #[test]
    fn test_merkle_tree_insert_updates_root() {
        let mut tree = MerkleTree::new(3).expect("failed to create tree");
        let initial_root = tree.root();

        let leaf1 = Fr::from(1);
        let index1 = tree.insert(leaf1).expect("failed to insert leaf");
        assert_eq!(index1, 0);
        assert_ne!(tree.root(), initial_root);

        let leaf2 = Fr::from(2);
        let index2 = tree.insert(leaf2).expect("failed to insert leaf");
        assert_eq!(index2, 1);
    }

    #[test]
    fn test_merkle_proof_verification() {
        let mut tree = MerkleTree::new(3).expect("failed to create tree");
        let leaf = Fr::from(123);
        let index = tree.insert(leaf).expect("failed to insert");

        let (path_elements, path_indices) = tree.get_proof(index).expect("failed to get proof");

        // Verify manually
        let mut current = leaf;
        for (element, index_bit) in path_elements.iter().zip(path_indices.iter()) {
            current = if *index_bit == 0 {
                hash_inputs(&[current, *element]).unwrap()
            } else {
                hash_inputs(&[*element, current]).unwrap()
            };
        }

        assert_eq!(current, tree.root(), "Proof verification failed");
    }

    #[test]
    fn test_merkle_tree_full_error() {
        // Create small tree of depth 1 (capacity 2)
        let mut tree = MerkleTree::new(1).expect("failed to create tree");
        tree.insert(Fr::from(1)).unwrap();
        tree.insert(Fr::from(2)).unwrap();

        let err = tree.insert(Fr::from(3));
        assert_eq!(err, Err(IdentityError::TreeFull));
    }

    #[test]
    fn test_invalid_index_error() {
        let mut tree = MerkleTree::new(3).expect("failed to create tree");
        tree.insert(Fr::from(1)).unwrap();

        assert!(tree.get_proof(0).is_ok());
        assert_eq!(tree.get_proof(1), Err(IdentityError::InvalidIndex(1)));
        assert_eq!(tree.get_proof(100), Err(IdentityError::InvalidIndex(100)));
    }
}
