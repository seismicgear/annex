-- A populated local channel is never replaced by first contact. Signed remote
-- history stays encrypted and separate until an explicit moderator operation
-- proves that every retained local revision has authoritative provenance.
CREATE TABLE channel_reconciliations (
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,operation_id TEXT NOT NULL,
 pseudonym_id TEXT NOT NULL,device_id TEXT NOT NULL,token_epoch INTEGER NOT NULL,
 peer_id INTEGER NOT NULL,authority_identity TEXT NOT NULL,authority_key_hex TEXT NOT NULL,
 authority_url TEXT NOT NULL,snapshot_sha256 TEXT NOT NULL,
 status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN('pending','conflict','complete','cancelled')),
 join_receipt_ciphertext TEXT,remote_join_accepted INTEGER NOT NULL DEFAULT 0 CHECK(remote_join_accepted IN(0,1)),after_seq INTEGER NOT NULL DEFAULT 0,after_hash TEXT NOT NULL DEFAULT '',
 head_seq INTEGER NOT NULL DEFAULT 0,head_hash TEXT NOT NULL DEFAULT '',
 result_json TEXT,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL,
 PRIMARY KEY(server_id,channel_id,operation_id),FOREIGN KEY(server_id) REFERENCES servers(id)
);
CREATE TABLE channel_reconciliation_records (
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,operation_id TEXT NOT NULL,event_seq INTEGER NOT NULL,
 event_hash TEXT NOT NULL,header_json TEXT NOT NULL,message_id TEXT,record_ciphertext TEXT,
 body_record INTEGER NOT NULL CHECK(body_record IN(0,1)),expires_at TEXT,
 PRIMARY KEY(server_id,channel_id,operation_id,event_seq),
 FOREIGN KEY(server_id,channel_id,operation_id) REFERENCES channel_reconciliations(server_id,channel_id,operation_id) ON DELETE CASCADE
);
CREATE INDEX reconciliation_message ON channel_reconciliation_records(server_id,channel_id,message_id);
-- These guards are created and removed inside the SAME final writer transaction.
-- They can never be committed by the service. Existing bytes remain referenced
-- until the verified replacement projection has been checked and committed.
CREATE TABLE channel_reconciliation_guards (
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,PRIMARY KEY(server_id,channel_id)
);
CREATE TABLE channel_reconciliation_departures (
 server_id INTEGER NOT NULL,channel_id TEXT NOT NULL,pseudonym_id TEXT NOT NULL,
 PRIMARY KEY(server_id,channel_id,pseudonym_id)
);
DROP TRIGGER cleanup_unreferenced_message_upload;
CREATE TRIGGER cleanup_unreferenced_message_upload AFTER DELETE ON message_uploads BEGIN
 DELETE FROM uploads WHERE server_id=OLD.server_id AND upload_id=OLD.upload_id
 AND NOT EXISTS(SELECT 1 FROM message_uploads WHERE server_id=OLD.server_id AND upload_id=OLD.upload_id)
 AND NOT EXISTS(SELECT 1 FROM channel_reconciliation_guards g WHERE g.server_id=uploads.server_id AND g.channel_id=uploads.channel_id);
END;
CREATE TRIGGER reconciliation_channel_deleted AFTER DELETE ON channels BEGIN
 DELETE FROM channel_reconciliation_records WHERE server_id=OLD.server_id AND channel_id=OLD.channel_id;
 UPDATE channel_reconciliations SET status='cancelled',join_receipt_ciphertext=NULL,result_json=NULL
 WHERE server_id=OLD.server_id AND channel_id=OLD.channel_id AND status<>'complete';
END;
