-- Migration 036 — federation receipt ledger (replay + freshness defence).
--
-- Why:
--   Before this migration, `receive_federated_message` only relied on
--   `messages.message_id UNIQUE` for replay defence. That gives same-
--   server idempotency but does NOT defend against:
--
--     * envelope replayed verbatim to a *different* peer server that
--       hasn't seen its message_id yet (cross-server replay),
--     * envelope replayed with the same message_id but a different
--       signed body (key compromise / supply-chain shenanigans),
--     * envelopes whose `created_at` is far enough in the past that
--       they should never have been delivered live (suppressed
--       envelope then released later),
--     * envelopes whose `created_at` is in the future (clock skew
--       or deliberate forward-dating).
--
--   `created_at` is part of the signed envelope (see
--   `message_signing_input` in
--   `crates/annex-server/src/services/federation_service.rs`), so it is
--   bound cryptographically — we just weren't *checking* it.
--
-- The receipt ledger gives us:
--
--   * Idempotent replay rejection per (remote, message_id).
--   * Inconsistency detection when the same message_id is re-presented
--     with a different `envelope_hash` (canonical SHA-256 of the
--     signed input).
--   * A target for the catch-up endpoint to write through (so
--     catch-up envelopes don't bypass the replay ledger).
--
-- The freshness window itself lives in config, not in this table —
-- see `Config::federation::freshness_window_seconds` and
-- `future_skew_seconds`. The table records *what was accepted*; the
-- decision of whether to accept is policy.

CREATE TABLE federation_message_receipts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    remote_instance_id INTEGER NOT NULL,
    -- The originating server's message_id. Bound to the envelope by
    -- its signature.
    message_id TEXT NOT NULL,
    -- SHA-256 of the canonical signing input (lowercase hex, 64
    -- chars). Two envelopes with the same message_id MUST hash to
    -- the same value; if they don't, one of them is a forgery
    -- attempt against a captured ID.
    envelope_hash TEXT NOT NULL,
    -- The originating server's claimed `created_at` (ISO 8601 from
    -- the envelope). Recorded for forensic / audit use even though
    -- the freshness decision was already made before insert.
    envelope_created_at TEXT NOT NULL,
    -- When this server *accepted* the envelope.
    received_at TEXT NOT NULL DEFAULT (datetime('now')),
    -- 'live' for envelopes delivered through the normal receive path,
    -- 'catchup' for envelopes delivered via the catch-up endpoint.
    -- This lets the freshness gate be strict on the live path while
    -- letting catch-up legitimately replay older history.
    delivery_mode TEXT NOT NULL DEFAULT 'live',
    UNIQUE (remote_instance_id, message_id),
    FOREIGN KEY (remote_instance_id) REFERENCES instances(id)
);

-- Lookup path is (remote_instance_id, message_id) — covered by the
-- UNIQUE constraint. Add an index on envelope_hash for forensic
-- queries ("did anyone else send this hash?") and one on
-- envelope_created_at for retention sweeps that purge ancient
-- receipts.
CREATE INDEX idx_federation_receipts_hash
    ON federation_message_receipts(envelope_hash);
CREATE INDEX idx_federation_receipts_created
    ON federation_message_receipts(envelope_created_at);
