-- Migration 035 — WebSocket / API command idempotency.
--
-- Why:
--   `clientRequestId` was previously echoed in the WS broadcast for
--   sender correlation, but never persisted. A replayed WS
--   `IncomingMessage::Message` (man-in-the-middle inside TLS, dropped
--   WS frame retried by the client, malicious replay of captured
--   frames inside a hijacked session) would insert a NEW message row
--   with a NEW server-generated `message_id`. For chat that is
--   annoying. For AI agents that act on chat content it is dangerous
--   (the "replay pay me $100" problem).
--
-- This migration adds the receiver-side ledger. The scope is
-- (server_id, sender_pseudonym, client_request_id) — a deliberately
-- per-sender scope. Two different senders may collide on
-- client_request_id without affecting each other. The
-- server_id qualifier keeps the table compatible with the
-- multi-tenant `servers` model.
--
-- The ledger stores the resulting `message_id` so a replayed send
-- can return the original message instead of creating a duplicate.
--
-- Rows are pruned by the retention sweep alongside their parent
-- messages (cascade is best-effort via separate retention; see
-- `crates/annex-server/src/retention.rs`). Hard cleanup is left to
-- a future maintenance pass.

CREATE TABLE message_request_ids (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    channel_id TEXT NOT NULL,
    sender_pseudonym TEXT NOT NULL,
    client_request_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (server_id, sender_pseudonym, client_request_id),
    FOREIGN KEY (server_id) REFERENCES servers(id)
);

-- Lookup path is (server, sender, client_request_id) — covered by the
-- UNIQUE constraint. Add an index on message_id for the resolve-by-id
-- direction used by the duplicate-return path.
CREATE INDEX idx_message_request_ids_message
    ON message_request_ids(message_id);
