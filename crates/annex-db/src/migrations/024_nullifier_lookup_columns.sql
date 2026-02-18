-- Add denormalized lookup columns to zk_nullifiers for O(1) pseudonym-to-commitment
-- resolution. Previously, find_commitment_for_pseudonym performed an O(N*M) scan
-- of all nullifiers × all identities, deriving and comparing hashes at runtime.
--
-- Addresses: C-06 (O(N*M) brute-force scan in federation message relay)

ALTER TABLE zk_nullifiers ADD COLUMN pseudonym_id TEXT;
ALTER TABLE zk_nullifiers ADD COLUMN commitment_hex TEXT;

-- Primary lookup: find commitment by pseudonym in O(1).
CREATE INDEX IF NOT EXISTS idx_zk_nullifiers_pseudonym
    ON zk_nullifiers(pseudonym_id)
    WHERE pseudonym_id IS NOT NULL;
