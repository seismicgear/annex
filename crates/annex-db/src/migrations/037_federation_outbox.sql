-- Migration 037 — durable federation outbox.
--
-- Why:
--   Before this migration, `relay_message` posted to every active peer
--   inside `tokio::spawn` and discarded the result. If the peer was
--   down, slow, partitioned, or returned a non-2xx response, the
--   envelope was lost. There was no retry queue, no delivery state,
--   and no catch-up path. Federation was a strict best-effort delivery
--   model that the architecture documents called out as a known gap.
--
-- This migration introduces a per-(peer, message) outbox row. The new
-- `relay_message` writes one row per active peer; a background worker
-- (see `crates/annex-server/src/background.rs::start_federation_outbox_task`)
-- drains pending rows with bounded exponential backoff. The receiver
-- side is already idempotent on the message receipt ledger introduced
-- in migration 036, so retries are safe.
--
-- Failure mode: when `attempts >= outbox_max_attempts`, the row moves
-- to `status='failed'` and is retained for audit / manual resend. The
-- worker does not delete failed rows; an operator with the admin
-- endpoint can `mark_pending` them after fixing the peer.

CREATE TABLE federation_outbox (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    peer_instance_id INTEGER NOT NULL,
    -- Originating-side message_id. (peer_instance_id, message_id)
    -- is unique so a duplicate enqueue (e.g. application bug) does
    -- not produce duplicate deliveries.
    message_id TEXT NOT NULL,
    -- The complete signed envelope JSON. We serialise once at enqueue
    -- time so the worker doesn't have to reconstruct or re-sign on
    -- retry — the receiver's freshness gate compares against the
    -- envelope's own `created_at`, not the retry time.
    envelope_json TEXT NOT NULL,
    -- 'pending' → in the retry rotation
    -- 'delivered' → 2xx received from peer (terminal)
    -- 'failed' → attempts exhausted (terminal, operator action)
    -- 'paused' → operator-suspended (terminal, operator action)
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    next_retry_at TEXT NOT NULL DEFAULT (datetime('now')),
    last_error TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (peer_instance_id, message_id),
    FOREIGN KEY (peer_instance_id) REFERENCES instances(id)
);

-- The worker's primary hot path is:
--   SELECT … FROM federation_outbox
--   WHERE status = 'pending' AND next_retry_at <= datetime('now')
--   ORDER BY next_retry_at ASC LIMIT N;
-- The composite index makes that O(log n).
CREATE INDEX idx_federation_outbox_status_retry
    ON federation_outbox(status, next_retry_at);
