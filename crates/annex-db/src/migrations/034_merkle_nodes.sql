-- Migration 034 — persisted Merkle internal nodes + root epochs.
--
-- Why:
--   001_identity.sql introduced `vrp_leaves` (the append-only commitment log)
--   and `vrp_roots` (a flat active/inactive table). On restart the server
--   reconstructed the tree by hashing every leaf in order, which scales
--   linearly with the identity set; and any in-flight proof against the
--   previous root was rejected the moment a single new identity was added,
--   producing the well-known concurrent-onboarding stale-proof regression.
--
-- This migration adds three tables. Existing tables are left untouched.
-- `vrp_leaves` continues to be the canonical append-only audit log.
-- `vrp_roots` continues to be populated by the legacy code path so the
-- existing api / middleware stays correct during the migration.
--
--   1. `vrp_merkle_nodes` — sparse storage for every internal node along
--      every path that has ever been touched. The Merkle tree restores
--      from this in O(touched_nodes) instead of O(leaves * depth).
--
--   2. `vrp_merkle_meta` — a singleton row capturing the current tree
--      depth, next insertion index, current root, and current epoch.
--      Loading this row is the fast-path of `MerkleTree::restore`.
--
--   3. `vrp_root_epochs` — the canonical root-history table with a grace
--      window (`accepted_until`) that lets clients submit proofs against
--      a recently-rotated root without immediate rejection. This solves
--      the stale-proof issue without losing the "is this root real"
--      check.

CREATE TABLE vrp_merkle_nodes (
    -- 0 = leaf level, depth = root level. (level, node_index) is unique
    -- across the whole tree.
    level INTEGER NOT NULL,
    node_index INTEGER NOT NULL,
    -- Canonical 64-character lowercase hex of the BN254 Fr at this node
    -- (see crates/annex-identity/src/zk.rs::fr_to_canonical_hex).
    hash_hex TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY (level, node_index)
);

-- Lookups go (level, node_index) -> hash_hex. The PK already covers that;
-- no extra index is needed because every read is a primary-key probe.

CREATE TABLE vrp_merkle_meta (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    tree_depth INTEGER NOT NULL,
    next_index INTEGER NOT NULL,
    current_root_hex TEXT NOT NULL,
    current_epoch INTEGER NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE vrp_root_epochs (
    -- Each row is one historical root. `root_epoch` is strictly increasing;
    -- gaps are not permitted. `root_hex` is unique across the table — a
    -- given Merkle state hashes to one canonical root.
    root_hex TEXT NOT NULL PRIMARY KEY,
    root_epoch INTEGER NOT NULL UNIQUE,
    leaf_count INTEGER NOT NULL,
    -- ISO-8601 / sqlite-`datetime('now')` strings throughout — keep
    -- consistent with the rest of the schema (see vrp_leaves.inserted_at,
    -- vrp_roots.created_at).
    active_from TEXT NOT NULL,
    -- NULL while this row is the current root.
    active_until TEXT,
    -- After `active_until`, proofs against this root remain accepted up to
    -- `accepted_until` (the grace window). NULL while the root is still
    -- active. Comparison with `datetime('now')` is the runtime acceptance
    -- gate.
    accepted_until TEXT
);

-- Useful when the verifier asks "is this root acceptable right now?" —
-- the root_hex PK gets the row in O(1); this index helps administrative
-- queries that walk history by epoch.
CREATE INDEX idx_vrp_root_epochs_epoch ON vrp_root_epochs(root_epoch);
