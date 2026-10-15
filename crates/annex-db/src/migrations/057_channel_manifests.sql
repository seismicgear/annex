-- Member signatures, independently verified by clients, authorize key recipients.
CREATE TABLE channel_manifest_state (
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,authority_identity TEXT NOT NULL,
 head_seq INTEGER NOT NULL CHECK(head_seq>0),head_hash TEXT NOT NULL,genesis_hash TEXT NOT NULL,
 projection_json TEXT NOT NULL,PRIMARY KEY(server_id,channel_id),
 FOREIGN KEY(channel_id) REFERENCES channels(channel_id) ON DELETE CASCADE
);
CREATE TABLE channel_manifest_events (
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,event_seq INTEGER NOT NULL,
 event_hash TEXT NOT NULL,event_json TEXT NOT NULL,
 PRIMARY KEY(server_id,channel_id,event_seq),UNIQUE(server_id,channel_id,event_hash),
 FOREIGN KEY(channel_id) REFERENCES channels(channel_id) ON DELETE CASCADE
);
CREATE TABLE channel_key_authorizations (
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,authorization_sha256 TEXT NOT NULL,
 receipt_json TEXT NOT NULL,PRIMARY KEY(server_id,channel_id,authorization_sha256),
 FOREIGN KEY(channel_id) REFERENCES channels(channel_id) ON DELETE CASCADE
);
ALTER TABLE channel_device_key_wraps ADD COLUMN authorization_sha256 TEXT;
ALTER TABLE channel_legacy_device_wraps ADD COLUMN authorization_sha256 TEXT;
ALTER TABLE channel_key_epochs ADD COLUMN manifest_hash TEXT;

-- Old ciphertext authenticates its original namespace. A stable authority pin
-- must never relabel an already sealed key/message as if it used that new AAD.
ALTER TABLE channel_key_epochs ADD COLUMN authority_namespace TEXT NOT NULL DEFAULT '';
UPDATE channel_key_epochs SET authority_namespace=COALESCE((SELECT s.authority_server FROM channel_key_state s WHERE s.server_id=channel_key_epochs.server_id AND s.channel_id=channel_key_epochs.channel_id),'');
ALTER TABLE channel_legacy_key_candidates ADD COLUMN authority_namespace TEXT NOT NULL DEFAULT '';
UPDATE channel_legacy_key_candidates SET authority_namespace=COALESCE((SELECT s.authority_server FROM channel_key_state s WHERE s.server_id=channel_legacy_key_candidates.server_id AND s.channel_id=channel_legacy_key_candidates.channel_id),'');
ALTER TABLE channel_key_wraps ADD COLUMN authority_namespace TEXT NOT NULL DEFAULT '';
UPDATE channel_key_wraps SET authority_namespace=COALESCE((SELECT s.authority_server FROM channel_key_state s WHERE s.server_id=channel_key_wraps.server_id AND s.channel_id=channel_key_wraps.channel_id),'');
CREATE TRIGGER channel_epoch_namespace_immutable BEFORE UPDATE OF authority_namespace ON channel_key_epochs
WHEN NEW.authority_namespace<>OLD.authority_namespace BEGIN SELECT RAISE(ABORT,'epoch ciphertext namespace is immutable'); END;
CREATE TRIGGER channel_legacy_namespace_immutable BEFORE UPDATE OF authority_namespace ON channel_legacy_key_candidates
WHEN NEW.authority_namespace<>OLD.authority_namespace BEGIN SELECT RAISE(ABORT,'legacy ciphertext namespace is immutable'); END;
