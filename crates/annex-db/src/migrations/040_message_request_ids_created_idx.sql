-- Migration 040 — index for the idempotency-ledger TTL sweep.
--
-- Why:
--   Migration 035 introduced `message_request_ids` (the WS command
--   idempotency ledger, ADR-0010) but deferred time-based eviction:
--   the table grew forever. The retention task now prunes rows whose
--   `created_at` is older than the configured idempotency TTL
--   (`server.idempotency_ttl_seconds`, default 7 days — see
--   `crates/annex-server/src/retention.rs`). The sweep's hot path is
--
--     DELETE FROM message_request_ids WHERE rowid IN (
--         SELECT rowid FROM message_request_ids
--         WHERE created_at < datetime('now', '-<ttl> seconds')
--         LIMIT <batch>
--     );
--
--   which needs an index on `created_at` to avoid a full table scan
--   on deployments that accumulated a large ledger before the sweep
--   existed.

CREATE INDEX idx_message_request_ids_created
    ON message_request_ids(created_at);
