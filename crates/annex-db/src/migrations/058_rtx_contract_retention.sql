-- A retained body has an immutable upper lifetime bound. Policy tightening can
-- shorten it; re-handshake, retries, restart and another relay cannot extend it.
ALTER TABLE rtx_bundles ADD COLUMN contract_json TEXT NOT NULL DEFAULT
    '{"knowledge_domains_allowed":null,"retention_seconds":null,"max_exchange_size":1048576}';
ALTER TABLE rtx_bundles ADD COLUMN expires_at_ms INTEGER;
CREATE INDEX idx_rtx_expiry ON rtx_bundles(server_id, expires_at_ms);
ALTER TABLE federation_outbox ADD COLUMN rtx_expires_at_ms INTEGER;
CREATE INDEX idx_rtx_outbox_expiry ON federation_outbox(rtx_expires_at_ms) WHERE envelope_kind='rtx';

-- Minimal replay state survives erasure of summaries, reasoning, tags, caveats,
-- provenance, governance rows and queued ciphertext. It cannot restore a body.
CREATE TABLE rtx_retention_tombstones (
    server_id INTEGER NOT NULL REFERENCES servers(id),
    bundle_id TEXT NOT NULL,
    content_digest TEXT NOT NULL,
    expired_at_ms INTEGER NOT NULL,
    PRIMARY KEY(server_id,bundle_id)
);
