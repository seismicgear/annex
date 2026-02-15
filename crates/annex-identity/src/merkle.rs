//! Poseidon Merkle Tree implementation.
//!
//! A binary Merkle tree using Poseidon hash function.
//! Supports append-only insertion and proof generation.

use crate::{poseidon::hash_inputs, IdentityError};
use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::HashMap;

/// Result of a preview insertion operation.
/// Tuple of: (leaf_index, new_root, updates_to_apply).
pub type InsertionPreview = (usize, Fr, Vec<((usize, usize), Fr)>);

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
        let (index, _, updates) = self.preview_insert(leaf)?;
        self.apply_updates(index + 1, updates);
        Ok(index)
    }

    /// Calculates the updates required to insert a leaf without modifying the tree.
    ///
    /// Returns `(index, new_root, updates)`.
    /// `updates` is a list of node keys and values that need to be inserted.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::TreeFull`] if the tree is full.
    /// Returns [`IdentityError::PoseidonError`] if hashing fails.
    pub fn preview_insert(&self, leaf: Fr) -> Result<InsertionPreview, IdentityError> {
        if self.next_index >= 1 << self.depth {
            return Err(IdentityError::TreeFull);
        }

        let index = self.next_index;
        let mut current_idx = index;
        let mut current_val = leaf;
        let mut updates = Vec::with_capacity(self.depth + 1);

        // Leaf update
        updates.push(((0, current_idx), current_val));

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
            updates.push(((level + 1, current_idx), current_val));
        }

        Ok((index, current_val, updates))
    }

    /// Applies updates calculated by `preview_insert`.
    ///
    /// Also updates `next_index`.
    pub fn apply_updates(&mut self, next_index: usize, updates: Vec<((usize, usize), Fr)>) {
        self.next_index = next_index;
        for (key, val) in updates {
            self.nodes.insert(key, val);
        }
    }

    /// Returns the current Merkle root.
    pub fn root(&self) -> Fr {
        *self
            .nodes
            .get(&(self.depth, 0))
            .unwrap_or(&self.zeros[self.depth])
    }

    /// Returns the current Merkle root as a big-endian hex string.
    pub fn root_hex(&self) -> String {
        let root = self.root();
        let bytes = root.into_bigint().to_bytes_be();
        hex::encode(bytes)
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
    /// Loads all leaves from `vrp_leaves` in order and inserts them into a fresh tree.
    /// Verifies that the final root matches the active root in `vrp_roots`.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::PoseidonError`] if hashing fails.
    /// Returns [`IdentityError::TreeFull`] if the tree cannot hold the persisted leaves.
    /// Returns [`IdentityError::InvalidHex`] if stored commitments or roots are invalid.
    /// Returns error if database query fails (wrapped).
    pub fn restore(conn: &Connection, depth: usize) -> Result<Self, IdentityError> {
        // 1. Create new empty tree
        let mut tree = Self::new(depth)?;

        // 2. Load leaves ordered by leaf_index
        let mut stmt = conn
            .prepare("SELECT commitment_hex FROM vrp_leaves ORDER BY leaf_index ASC")
            .map_err(IdentityError::DatabaseError)?;

        let leaf_iter = stmt
            .query_map([], |row| {
                let hex: String = row.get(0)?;
                Ok(hex)
            })
            .map_err(IdentityError::DatabaseError)?;

        for leaf_result in leaf_iter {
            let hex_str = leaf_result.map_err(IdentityError::DatabaseError)?;
            // Convert hex to Fr
            let bytes = hex::decode(&hex_str).map_err(|_| IdentityError::InvalidHex)?;
            let leaf = Fr::from_be_bytes_mod_order(&bytes);
            tree.insert(leaf)?;
        }

        // 3. Verify root
        let stored_root_hex: Option<String> = conn
            .query_row(
                "SELECT root_hex FROM vrp_roots WHERE active = 1 ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(IdentityError::DatabaseError)?;

        if let Some(stored_hex) = stored_root_hex {
            let current_root_bytes = tree.root().into_bigint().to_bytes_be();
            let current_root_hex = hex::encode(current_root_bytes);

            if current_root_hex != stored_hex {
                tracing::warn!(
                    "Merkle root mismatch! Stored: {}, Computed: {}",
                    stored_hex,
                    current_root_hex
                );
                // We prioritize computed root.
            }
        }

        Ok(tree)
    }

    /// Persists a leaf and the current root to the database without starting a transaction.
    ///
    /// Use this when you are already inside a transaction or need fine-grained control.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::DatabaseError`] if SQL execution fails.
    pub fn persist_leaf_and_root(
        &self,
        conn: &Connection,
        index: usize,
        leaf: Fr,
        root: Fr,
    ) -> Result<(), IdentityError> {
        let leaf_bytes = leaf.into_bigint().to_bytes_be();
        let leaf_hex = hex::encode(leaf_bytes);
        let root_bytes = root.into_bigint().to_bytes_be();
        let root_hex = hex::encode(root_bytes);

        // Note: rusqlite::Connection executes directly.
        // If called with a Transaction object (which Derefs to Connection), it works within that transaction.

        conn.execute(
            "INSERT INTO vrp_leaves (leaf_index, commitment_hex) VALUES (?1, ?2)",
            params![index, leaf_hex],
        )
        .map_err(IdentityError::DatabaseError)?;

        // Mark previous root as inactive
        conn.execute("UPDATE vrp_roots SET active = 0 WHERE active = 1", [])
            .map_err(IdentityError::DatabaseError)?;

        // Insert new active root
        conn.execute(
            "INSERT INTO vrp_roots (root_hex, active) VALUES (?1, 1)",
            params![root_hex],
        )
        .map_err(IdentityError::DatabaseError)?;

        Ok(())
    }

    /// Inserts a leaf and persists it to the database, managing its own transaction.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::DatabaseError`] if transaction or SQL fails.
    pub fn insert_and_persist(
        &mut self,
        conn: &mut Connection,
        leaf: Fr,
    ) -> Result<usize, IdentityError> {
        let index = self.insert(leaf)?;
        let root = self.root();

        let tx = conn.transaction().map_err(IdentityError::DatabaseError)?;
        self.persist_leaf_and_root(&tx, index, leaf, root)?;
        tx.commit().map_err(IdentityError::DatabaseError)?;

        Ok(index)
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
