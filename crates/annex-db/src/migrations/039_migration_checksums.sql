-- Migration 039 — migration integrity checksums.
--
-- Why:
--   `_annex_migrations` previously recorded only `(name, applied_at)`.
--   That meant a silently-edited migration SQL (e.g. someone fixed a
--   typo in 010_messages.sql on a live tree) would not be detected:
--   on next boot the runner would see the name in the table and skip
--   re-applying. The invariants doc admits this — "no tooling
--   enforcement yet, humans must hold the line."
--
-- This migration extends the tracking table with `sha256_hex` and
-- `ordinal`. The runner now:
--   * verifies that every already-applied migration's recorded
--     SHA-256 matches the embedded source on boot;
--   * detects duplicate ordinals (two migrations claiming the same
--     number);
--   * fails loudly with `MigrationError::ChecksumMismatch` rather
--     than silently accepting drift.
--
-- Existing installations already have rows without checksums. The
-- runner backfills `sha256_hex` for those rows on first boot under
-- this migration (using the embedded SQL it would have applied),
-- treating that as the trusted baseline. After backfill, any future
-- edit to a committed migration trips the gate.

ALTER TABLE _annex_migrations ADD COLUMN sha256_hex TEXT;
ALTER TABLE _annex_migrations ADD COLUMN ordinal INTEGER;

-- Lookups during the integrity check are by name; the existing UNIQUE
-- on `name` covers that. Add an index on ordinal for the
-- duplicate-detection scan.
CREATE INDEX IF NOT EXISTS idx_annex_migrations_ordinal
    ON _annex_migrations(ordinal);
