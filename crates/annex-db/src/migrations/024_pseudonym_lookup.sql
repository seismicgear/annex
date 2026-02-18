-- Migration 024: Add pseudonym_id and commitment_hex columns to zk_nullifiers
-- for O(1) indexed lookup instead of O(N*M) brute-force scan.
--
-- Before this migration, find_commitment_for_pseudonym() performs:
--   SELECT * FROM zk_nullifiers  (full scan)
--   For each row: derive_pseudonym_id()
--   SELECT * FROM vrp_identities (full scan per match)
--   For each: derive_nullifier_hex()
-- This is O(N*M) and degrades over 100 years of operation.
--
-- After: SELECT commitment_hex, topic FROM zk_nullifiers WHERE pseudonym_id = ?
-- with an index on pseudonym_id, this is O(1).

ALTER TABLE zk_nullifiers ADD COLUMN pseudonym_id TEXT;
ALTER TABLE zk_nullifiers ADD COLUMN commitment_hex TEXT;

CREATE INDEX IF NOT EXISTS idx_zk_nullifiers_pseudonym
    ON zk_nullifiers(pseudonym_id)
    WHERE pseudonym_id IS NOT NULL;
