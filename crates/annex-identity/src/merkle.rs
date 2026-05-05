//! Poseidon Merkle Tree implementation.
//!
//! A binary Merkle tree using Poseidon hash function.
//! Supports append-only insertion and proof generation.

use crate::zk::fr_to_canonical_hex;
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
        // Use checked shift to avoid panic on depth >= 64 (or >= 32 on 32-bit).
        let capacity = 1usize.checked_shl(self.depth as u32).unwrap_or(usize::MAX);
        if self.next_index >= capacity {
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

    /// Returns the current Merkle root as a canonical 64-character lowercase
    /// hex string (see [`fr_to_canonical_hex`]).
    pub fn root_hex(&self) -> String {
        fr_to_canonical_hex(self.root())
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
    /// Fast path (production): the singleton row in `vrp_merkle_meta` plus
    /// every node in `vrp_merkle_nodes`. This is O(touched_nodes) and lets
    /// the server boot without hashing every historical leaf.
    ///
    /// Fallback path (legacy databases or first boot after migration 034):
    /// when no `vrp_merkle_meta` row exists, the tree is reconstructed by
    /// replaying `vrp_leaves` in `leaf_index` order — same behaviour as
    /// before this migration. The freshly-rebuilt nodes are then persisted
    /// so the next restart takes the fast path.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::MerkleTreeDepthMismatch`] if the persisted
    /// `tree_depth` differs from the requested depth.
    /// Returns [`IdentityError::MerkleRootMismatch`] if the persisted node
    /// state hashes to a root other than the one recorded in
    /// `vrp_merkle_meta` (corruption signal — refuse to serve).
    /// Returns [`IdentityError::PoseidonError`] / [`IdentityError::TreeFull`] /
    /// [`IdentityError::InvalidHex`] / [`IdentityError::DatabaseError`] for
    /// the legacy rebuild path.
    pub fn restore(conn: &Connection, depth: usize) -> Result<Self, IdentityError> {
        if let Some(meta) = load_meta(conn)? {
            return Self::restore_from_meta(conn, depth, meta);
        }

        // Legacy / fresh-DB path: rebuild from leaves and seed the new
        // persisted-state tables. Subsequent restarts will hit the fast
        // path above.
        let tree = Self::rebuild_from_leaves(conn, depth)?;

        // Cross-check against `vrp_roots` (legacy active-root table) so a
        // database that already has rows there but no merkle_nodes can't
        // bootstrap into a divergent state.
        let stored_root_hex: Option<String> = conn
            .query_row(
                "SELECT root_hex FROM vrp_roots WHERE active = 1 ORDER BY created_at DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(IdentityError::DatabaseError)?;

        if let Some(stored_hex) = stored_root_hex {
            let current_root_hex = fr_to_canonical_hex(tree.root());
            let stored_normalised = stored_hex.to_ascii_lowercase();
            if current_root_hex != stored_normalised {
                return Err(IdentityError::MerkleRootMismatch {
                    stored: stored_hex,
                    computed: current_root_hex,
                });
            }
        }

        // Seed merkle_nodes + meta + root_epochs so the next restart takes
        // the fast path. Skipped if the conn isn't a transaction-capable
        // connection — the legacy persistence path below tolerates either.
        tree.write_persisted_state(conn, /* epoch */ tree.next_index as i64)?;
        Ok(tree)
    }

    /// Fast-path restore: load the in-memory state from `vrp_merkle_meta`
    /// and `vrp_merkle_nodes`. Validates depth and root.
    fn restore_from_meta(
        conn: &Connection,
        depth: usize,
        meta: PersistedMeta,
    ) -> Result<Self, IdentityError> {
        if meta.tree_depth != depth {
            return Err(IdentityError::MerkleTreeDepthMismatch {
                stored: meta.tree_depth,
                configured: depth,
            });
        }

        let mut tree = Self::new(depth)?;
        tree.next_index = meta.next_index;

        let mut stmt = conn
            .prepare("SELECT level, node_index, hash_hex FROM vrp_merkle_nodes")
            .map_err(IdentityError::DatabaseError)?;
        let rows = stmt
            .query_map([], |row| {
                let level: i64 = row.get(0)?;
                let node_index: i64 = row.get(1)?;
                let hex: String = row.get(2)?;
                Ok((level as usize, node_index as usize, hex))
            })
            .map_err(IdentityError::DatabaseError)?;

        for r in rows {
            let (level, node_index, hex_str) = r.map_err(IdentityError::DatabaseError)?;
            if level > depth {
                // Malformed row — caller must run repair_persisted_nodes.
                return Err(IdentityError::InvalidHex);
            }
            let fr = parse_canonical_field_hex(&hex_str)?;
            tree.nodes.insert((level, node_index), fr);
        }

        let computed = fr_to_canonical_hex(tree.root());
        let expected = meta.current_root_hex.to_ascii_lowercase();
        if computed != expected {
            return Err(IdentityError::MerkleRootMismatch {
                stored: meta.current_root_hex,
                computed,
            });
        }

        Ok(tree)
    }

    /// Replays every leaf from `vrp_leaves` (audit / repair path).
    ///
    /// This is the slow O(leaves * depth) reconstruction. It is the
    /// authoritative source-of-truth check used both as the legacy boot
    /// fallback and by [`MerkleTree::audit_against_leaves`].
    pub fn rebuild_from_leaves(conn: &Connection, depth: usize) -> Result<Self, IdentityError> {
        let mut tree = Self::new(depth)?;

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
            let bytes = hex::decode(&hex_str).map_err(|_| IdentityError::InvalidHex)?;
            let leaf = Fr::from_be_bytes_mod_order(&bytes);
            let roundtrip = leaf.into_bigint().to_bytes_be();
            let mut padded = vec![0u8; 32usize.saturating_sub(bytes.len())];
            padded.extend_from_slice(&bytes);
            if padded.len() > 32 || padded != roundtrip {
                return Err(IdentityError::InvalidHex);
            }
            tree.insert(leaf)?;
        }

        Ok(tree)
    }

    /// Audits the persisted node + meta state by re-deriving the root from
    /// the leaves, returning the recomputed root hex. Callers compare it
    /// against `vrp_merkle_meta.current_root_hex` to confirm integrity.
    pub fn audit_against_leaves(conn: &Connection, depth: usize) -> Result<String, IdentityError> {
        let rebuilt = Self::rebuild_from_leaves(conn, depth)?;
        Ok(fr_to_canonical_hex(rebuilt.root()))
    }

    /// Bootstraps `vrp_merkle_nodes` + `vrp_merkle_meta` + `vrp_root_epochs`
    /// from the in-memory state. Used:
    ///   - by the fallback path of `restore` to seed the tables on the
    ///     first boot after migration 034 lands;
    ///   - by `repair_persisted_nodes` when an operator wants to rebuild
    ///     the persisted state from leaves.
    fn write_persisted_state(
        &self,
        conn: &Connection,
        starting_epoch: i64,
    ) -> Result<(), IdentityError> {
        // Wipe any previous merkle_nodes; we are about to re-seed from
        // scratch. The `vrp_merkle_meta` row is a singleton, so we use
        // INSERT OR REPLACE.
        conn.execute("DELETE FROM vrp_merkle_nodes", [])
            .map_err(IdentityError::DatabaseError)?;
        for ((level, idx), fr) in &self.nodes {
            conn.execute(
                "INSERT INTO vrp_merkle_nodes (level, node_index, hash_hex) VALUES (?1, ?2, ?3)",
                params![*level as i64, *idx as i64, fr_to_canonical_hex(*fr)],
            )
            .map_err(IdentityError::DatabaseError)?;
        }

        let root_hex = fr_to_canonical_hex(self.root());
        conn.execute(
            "INSERT OR REPLACE INTO vrp_merkle_meta \
             (id, tree_depth, next_index, current_root_hex, current_epoch, updated_at) \
             VALUES (1, ?1, ?2, ?3, ?4, datetime('now'))",
            params![
                self.depth as i64,
                self.next_index as i64,
                root_hex,
                starting_epoch
            ],
        )
        .map_err(IdentityError::DatabaseError)?;

        // Idempotent epoch seed: if no row exists for this root yet, write
        // one as the active row. Existing rows are left alone.
        conn.execute(
            "INSERT OR IGNORE INTO vrp_root_epochs \
             (root_hex, root_epoch, leaf_count, active_from, active_until, accepted_until) \
             VALUES (?1, ?2, ?3, datetime('now'), NULL, NULL)",
            params![root_hex, starting_epoch, self.next_index as i64],
        )
        .map_err(IdentityError::DatabaseError)?;
        Ok(())
    }

    /// Operator repair: rebuild the tree from `vrp_leaves`, write it back
    /// into `vrp_merkle_nodes` + `vrp_merkle_meta` (epoch-preserving), and
    /// return the recomputed root hex.
    ///
    /// Use this after manual schema surgery, after restoring from a backup,
    /// or whenever the audit signal in [`Self::audit_against_leaves`]
    /// disagrees with `vrp_merkle_meta.current_root_hex`.
    pub fn repair_persisted_nodes(
        conn: &mut Connection,
        depth: usize,
    ) -> Result<String, IdentityError> {
        let rebuilt = Self::rebuild_from_leaves(conn, depth)?;
        let starting_epoch = load_meta(conn)?
            .map(|m| m.current_epoch)
            .unwrap_or(rebuilt.next_index as i64);
        let tx = conn.transaction().map_err(IdentityError::DatabaseError)?;
        rebuilt.write_persisted_state(&tx, starting_epoch)?;
        tx.commit().map_err(IdentityError::DatabaseError)?;
        Ok(fr_to_canonical_hex(rebuilt.root()))
    }

    /// Persists a leaf, the current root, and the *touched node path* to
    /// the database without starting a transaction.
    ///
    /// Use this when you are already inside a transaction or need
    /// fine-grained control. The caller MUST pass the `updates` vector
    /// produced by [`Self::preview_insert`] — it represents the (level,
    /// index) → hash entries along the inserted leaf's path. Persisting
    /// only the path is the whole point of `vrp_merkle_nodes`: a fresh
    /// boot reads exactly the nodes it needs from the table instead of
    /// re-hashing the entire leaf set.
    ///
    /// On a fully-fresh DB (no `vrp_merkle_meta` row) this also seeds the
    /// initial epoch metadata so the next call has somewhere to bump.
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
        updates: &[((usize, usize), Fr)],
    ) -> Result<(), IdentityError> {
        let leaf_hex = fr_to_canonical_hex(leaf);
        let root_hex = fr_to_canonical_hex(root);

        // 1. Append-only audit log — vrp_leaves stays the source of truth
        // even after the move to persisted nodes.
        conn.execute(
            "INSERT INTO vrp_leaves (leaf_index, commitment_hex) VALUES (?1, ?2)",
            params![index, leaf_hex],
        )
        .map_err(IdentityError::DatabaseError)?;

        // 2. Legacy root table — kept untouched so the existing api / WS
        // hot path that reads `vrp_roots WHERE active = 1` continues to
        // work during the migration window.
        conn.execute("UPDATE vrp_roots SET active = 0 WHERE active = 1", [])
            .map_err(IdentityError::DatabaseError)?;
        conn.execute(
            "INSERT INTO vrp_roots (root_hex, active) VALUES (?1, 1)",
            params![root_hex],
        )
        .map_err(IdentityError::DatabaseError)?;

        // 3. Persist only the *path* nodes that changed. Every entry in
        // `updates` is one (level, node_index) -> hash assignment along
        // the leaf's path to the root. PK collisions update in place via
        // INSERT OR REPLACE.
        for ((level, node_index), fr) in updates {
            conn.execute(
                "INSERT OR REPLACE INTO vrp_merkle_nodes \
                 (level, node_index, hash_hex, updated_at) \
                 VALUES (?1, ?2, ?3, datetime('now'))",
                params![*level as i64, *node_index as i64, fr_to_canonical_hex(*fr)],
            )
            .map_err(IdentityError::DatabaseError)?;
        }

        // 4. Bump epoch + retire the previous root in `vrp_root_epochs`.
        // The previous active row's `active_until` is set to now and
        // `accepted_until` to now + GRACE so an in-flight client whose
        // proof was generated against the previous root still verifies
        // for a short window.
        let new_epoch = match load_meta(conn)? {
            Some(m) => m.current_epoch + 1,
            None => 0,
        };

        conn.execute(
            "UPDATE vrp_root_epochs \
             SET active_until = datetime('now'), \
                 accepted_until = datetime('now', ?1) \
             WHERE active_until IS NULL",
            params![format!("+{} seconds", ROOT_EPOCH_GRACE_SECONDS)],
        )
        .map_err(IdentityError::DatabaseError)?;

        conn.execute(
            "INSERT OR REPLACE INTO vrp_root_epochs \
             (root_hex, root_epoch, leaf_count, active_from, active_until, accepted_until) \
             VALUES (?1, ?2, ?3, datetime('now'), NULL, NULL)",
            params![root_hex, new_epoch, (index + 1) as i64],
        )
        .map_err(IdentityError::DatabaseError)?;

        // 5. Update the singleton meta row.
        conn.execute(
            "INSERT OR REPLACE INTO vrp_merkle_meta \
             (id, tree_depth, next_index, current_root_hex, current_epoch, updated_at) \
             VALUES (1, ?1, ?2, ?3, ?4, datetime('now'))",
            params![self.depth as i64, (index + 1) as i64, root_hex, new_epoch],
        )
        .map_err(IdentityError::DatabaseError)?;

        Ok(())
    }

    /// Inserts a leaf and persists it to the database, managing its own transaction.
    ///
    /// Uses `preview_insert` to calculate changes without modifying the in-memory
    /// tree. The tree is only updated after the database transaction commits
    /// successfully, preventing divergence between in-memory and persisted state.
    ///
    /// # Errors
    ///
    /// Returns [`IdentityError::TreeFull`] if the tree is full.
    /// Returns [`IdentityError::PoseidonError`] if hashing fails.
    /// Returns [`IdentityError::DatabaseError`] if transaction or SQL fails.
    pub fn insert_and_persist(
        &mut self,
        conn: &mut Connection,
        leaf: Fr,
    ) -> Result<usize, IdentityError> {
        // Preview changes without mutating the tree
        let (index, new_root, updates) = self.preview_insert(leaf)?;

        // Persist inside a transaction so vrp_leaves, vrp_roots,
        // vrp_merkle_nodes, vrp_root_epochs, and vrp_merkle_meta all
        // commit atomically. This is the only operation that ratchets
        // `next_index`; concurrent registrations serialise on the SQLite
        // write lock here, so two callers can never observe the same
        // `next_index` value.
        let tx = conn.transaction().map_err(IdentityError::DatabaseError)?;
        self.persist_leaf_and_root(&tx, index, leaf, new_root, &updates)?;
        tx.commit().map_err(IdentityError::DatabaseError)?;

        // Only apply to in-memory tree after successful commit
        self.apply_updates(index + 1, updates);

        Ok(index)
    }
}

/// Default grace window for accepting proofs against the previous root.
/// Five minutes is enough to cover client-side proof-generation latency
/// (~ a few seconds of WASM Groth16) plus a generous margin for slow
/// devices and network round-trips. Tuneable: tests override directly.
pub const ROOT_EPOCH_GRACE_SECONDS: i64 = 300;

/// Snapshot of the singleton row in `vrp_merkle_meta`.
#[derive(Debug, Clone)]
pub struct PersistedMeta {
    pub tree_depth: usize,
    pub next_index: usize,
    pub current_root_hex: String,
    pub current_epoch: i64,
}

/// Reads the singleton meta row, if present. Returns `Ok(None)` when the
/// table exists but is empty (fresh DB after migration 034 ran but before
/// any insert) or pre-034 databases.
pub fn load_meta(conn: &Connection) -> Result<Option<PersistedMeta>, IdentityError> {
    let row: Option<(i64, i64, String, i64)> = conn
        .query_row(
            "SELECT tree_depth, next_index, current_root_hex, current_epoch \
             FROM vrp_merkle_meta WHERE id = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()
        .map_err(IdentityError::DatabaseError)?;
    Ok(row.map(|(d, n, r, e)| PersistedMeta {
        tree_depth: d as usize,
        next_index: n as usize,
        current_root_hex: r,
        current_epoch: e,
    }))
}

/// Returns whether `root_hex` is currently acceptable as the basis for a
/// membership proof.
///
/// A root is acceptable when a row exists in `vrp_root_epochs` AND
/// either:
///   - `active_until IS NULL` (it is the current root), or
///   - `datetime('now') <= accepted_until` (the grace window after
///     rotation has not yet expired).
pub fn is_root_acceptable(conn: &Connection, root_hex: &str) -> Result<bool, IdentityError> {
    let normalised = root_hex.to_ascii_lowercase();
    let acceptable: Option<bool> = conn
        .query_row(
            "SELECT \
                CASE \
                    WHEN active_until IS NULL THEN 1 \
                    WHEN accepted_until IS NOT NULL AND datetime('now') <= accepted_until THEN 1 \
                    ELSE 0 \
                END \
             FROM vrp_root_epochs WHERE root_hex = ?1",
            params![normalised],
            |row| row.get::<_, i64>(0).map(|v| v != 0),
        )
        .optional()
        .map_err(IdentityError::DatabaseError)?;
    Ok(acceptable.unwrap_or(false))
}

fn parse_canonical_field_hex(s: &str) -> Result<Fr, IdentityError> {
    if s.len() != 64 {
        return Err(IdentityError::InvalidHex);
    }
    let bytes = hex::decode(s).map_err(|_| IdentityError::InvalidHex)?;
    let fr = Fr::from_be_bytes_mod_order(&bytes);
    let roundtrip = fr.into_bigint().to_bytes_be();
    if roundtrip != bytes {
        return Err(IdentityError::InvalidHex);
    }
    Ok(fr)
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

    #[test]
    fn preview_insert_does_not_mutate_tree() {
        let tree = MerkleTree::new(3).expect("failed to create tree");
        let root_before = tree.root();
        let next_idx_before = tree.next_index;

        let _preview = tree
            .preview_insert(Fr::from(42))
            .expect("preview should succeed");

        assert_eq!(
            tree.root(),
            root_before,
            "preview_insert must not change root"
        );
        assert_eq!(
            tree.next_index, next_idx_before,
            "preview_insert must not change next_index"
        );
    }

    #[test]
    fn insert_and_persist_rolls_back_tree_on_db_failure() {
        // Use a connection WITHOUT the required tables to simulate DB failure
        let mut conn = rusqlite::Connection::open_in_memory().unwrap();
        let mut tree = MerkleTree::new(3).expect("failed to create tree");
        let root_before = tree.root();

        let result = tree.insert_and_persist(&mut conn, Fr::from(99));
        assert!(result.is_err(), "should fail due to missing table");

        // Tree state must remain unchanged
        assert_eq!(
            tree.root(),
            root_before,
            "tree must not be modified on DB failure"
        );
        assert_eq!(
            tree.next_index, 0,
            "next_index must not advance on DB failure"
        );
    }

    #[test]
    fn checked_capacity_handles_extreme_depth() {
        // This verifies that the checked_shl prevents panic for large depths.
        // We can't actually create a tree with depth 64 (memory), but we can
        // test the capacity calculation logic directly.
        let capacity = 1usize.checked_shl(64).unwrap_or(usize::MAX);
        assert_eq!(
            capacity,
            usize::MAX,
            "depth 64 should saturate to usize::MAX"
        );
    }

    #[test]
    fn root_hex_of_empty_tree_is_64_chars() {
        let tree = MerkleTree::new(20).expect("failed to create tree");
        let h = tree.root_hex();
        assert_eq!(
            h.len(),
            64,
            "empty-tree root_hex must be canonical 64-char hex"
        );
        assert!(
            h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "root_hex must be lowercase hex: got {h}"
        );
    }

    #[test]
    fn root_hex_after_inserts_is_64_chars() {
        let mut tree = MerkleTree::new(5).expect("failed to create tree");
        for v in 1u64..=4 {
            tree.insert(Fr::from(v)).expect("insert");
            let h = tree.root_hex();
            assert_eq!(
                h.len(),
                64,
                "root_hex must remain 64 chars after insert (after {v} insertions): got {h}"
            );
        }
    }

    #[test]
    fn proof_path_elements_serialise_to_64_chars_each() {
        // Build a small tree and compute a proof for a leaf, then exercise the
        // same canonical serialisation registry.rs uses on the wire.
        let mut tree = MerkleTree::new(5).expect("failed to create tree");
        let leaf = Fr::from(123u64);
        let index = tree.insert(leaf).expect("insert");
        let (path_elements, _path_indices) = tree.get_proof(index).expect("proof");
        assert_eq!(path_elements.len(), 5, "depth-5 tree => 5 path elements");
        for fr in path_elements {
            let h = crate::zk::fr_to_canonical_hex(fr);
            assert_eq!(h.len(), 64, "path element hex must be 64 chars: got {h}");
            assert!(
                h.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
                "path element must be lowercase hex: got {h}"
            );
        }
    }
}
