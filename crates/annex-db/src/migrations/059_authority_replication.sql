-- Typed human membership is deliberately separate from agent VRP attestation.
-- It conveys current origin membership, never channel or moderator admission.
ALTER TABLE channel_authority_events ADD COLUMN removal_certificate_json TEXT;
-- Exact control receipts allow departure retries without disclosing a newer
-- member directory to someone whose membership was removed by that operation.
ALTER TABLE channel_manifest_events ADD COLUMN response_json TEXT;
CREATE TABLE federated_human_memberships (
 server_id INTEGER NOT NULL, peer_instance_id INTEGER NOT NULL,
 channel_id TEXT NOT NULL, pseudonym_id TEXT NOT NULL, device_id TEXT NOT NULL,
 certificate_sha256 TEXT NOT NULL, origin_token_epoch INTEGER NOT NULL,
 expires_at INTEGER NOT NULL, request_hash TEXT NOT NULL,
 PRIMARY KEY(server_id,peer_instance_id,channel_id,pseudonym_id,device_id),
 FOREIGN KEY(server_id) REFERENCES servers(id),
 FOREIGN KEY(peer_instance_id) REFERENCES instances(id)
);
CREATE TABLE channel_replica_readers (
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,pseudonym_id TEXT NOT NULL,
 device_id TEXT NOT NULL,token_epoch INTEGER NOT NULL,
 next_attempt_at INTEGER NOT NULL DEFAULT 0,last_error TEXT,
 revoked_at INTEGER,updated_at INTEGER NOT NULL,
 PRIMARY KEY(server_id,channel_id,pseudonym_id,device_id),
 FOREIGN KEY(server_id,channel_id) REFERENCES channel_authorities(server_id,channel_id)
);
CREATE TABLE channel_authority_outgoing (
 id INTEGER PRIMARY KEY AUTOINCREMENT,server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,
 pseudonym_id TEXT NOT NULL,device_id TEXT NOT NULL,token_epoch INTEGER NOT NULL,
 operation_id TEXT NOT NULL,operation_hash TEXT NOT NULL,operation_ciphertext TEXT,
 status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN('pending','accepted','failed','cancelled')),
 accepted_seq INTEGER,accepted_hash TEXT,departure_receipt_ciphertext TEXT,last_error TEXT,attempts INTEGER NOT NULL DEFAULT 0,
 next_attempt_at INTEGER NOT NULL DEFAULT 0,created_at INTEGER NOT NULL,
 UNIQUE(server_id,channel_id,pseudonym_id,operation_id),
 FOREIGN KEY(server_id,channel_id) REFERENCES channel_authorities(server_id,channel_id),
 CHECK((status='pending')=(operation_ciphertext IS NOT NULL))
);
CREATE INDEX authority_outgoing_due ON channel_authority_outgoing(server_id,status,next_attempt_at,id);
CREATE INDEX replica_readers_due ON channel_replica_readers(server_id,revoked_at,next_attempt_at);
-- No queued ciphertext is retained after the local message/channel authority
-- has told this replica that its access or the channel itself was removed.
CREATE TRIGGER replica_channel_queue_deleted AFTER DELETE ON channels
BEGIN
 UPDATE channel_authority_outgoing SET status='cancelled',operation_ciphertext=NULL,last_error='channel deleted'
 WHERE server_id=OLD.server_id AND channel_id=OLD.channel_id AND status='pending';
 UPDATE channel_authority_outgoing SET departure_receipt_ciphertext=NULL
 WHERE server_id=OLD.server_id AND channel_id=OLD.channel_id;
 UPDATE channel_replica_readers SET revoked_at=unixepoch(),last_error='channel deleted'
 WHERE server_id=OLD.server_id AND channel_id=OLD.channel_id;
END;
