-- Track verification state for federated identities
ALTER TABLE federated_identities ADD COLUMN root_hex_at_verification TEXT DEFAULT '';
ALTER TABLE federated_identities ADD COLUMN last_verified_at TEXT DEFAULT (datetime('now'));
