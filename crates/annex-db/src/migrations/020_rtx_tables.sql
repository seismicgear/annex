-- RTX (Reflection Transfer Exchange) tables for Phase 9.
--
-- rtx_bundles: stores published ReflectionSummaryBundles.
-- rtx_subscriptions: agent subscription filters for bundle delivery (used by 9.3).
-- rtx_transfer_log: auditable log of all bundle transfers (used by 9.5).

CREATE TABLE IF NOT EXISTS rtx_bundles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    bundle_id TEXT NOT NULL UNIQUE,
    source_pseudonym TEXT NOT NULL,
    source_server TEXT NOT NULL,
    domain_tags_json TEXT NOT NULL,
    summary TEXT NOT NULL,
    reasoning_chain TEXT,
    caveats_json TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    signature TEXT NOT NULL,
    vrp_handshake_ref TEXT NOT NULL,
    stored_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (server_id) REFERENCES servers(id)
);

CREATE INDEX IF NOT EXISTS idx_rtx_bundles_source
    ON rtx_bundles(server_id, source_pseudonym);

CREATE INDEX IF NOT EXISTS idx_rtx_bundles_stored_at
    ON rtx_bundles(server_id, stored_at);

CREATE TABLE IF NOT EXISTS rtx_subscriptions (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    subscriber_pseudonym TEXT NOT NULL,
    domain_filters_json TEXT NOT NULL DEFAULT '[]',
    accept_federated INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (server_id) REFERENCES servers(id),
    UNIQUE (server_id, subscriber_pseudonym)
);

CREATE TABLE IF NOT EXISTS rtx_transfer_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    bundle_id TEXT NOT NULL,
    source_pseudonym TEXT NOT NULL,
    destination_pseudonym TEXT,
    transfer_scope_applied TEXT NOT NULL,
    redactions_applied TEXT,
    transferred_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (server_id) REFERENCES servers(id)
);

CREATE INDEX IF NOT EXISTS idx_rtx_transfer_log_bundle
    ON rtx_transfer_log(bundle_id);
