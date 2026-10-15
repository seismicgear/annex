-- Versioned ordinary attachments may contain fewer than 28 bytes. Preserve all
-- existing transfer IDs, leases, offsets, tombstones and cleanup obligations.
-- No upload/file rows are deleted by this schema upgrade.
DROP TRIGGER notify_replica_upload_status;
DROP TRIGGER cancel_deleted_replica_upload;
CREATE TABLE federated_upload_transfers_v2 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL REFERENCES servers(id),
    upload_id TEXT NOT NULL UNIQUE,
    authority_identity TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    message_id TEXT NOT NULL,
    accepted_event_seq INTEGER NOT NULL CHECK(accepted_event_seq > 0),
    accepted_event_hash TEXT NOT NULL,
    accepted_json TEXT NOT NULL,
    origin_identity TEXT NOT NULL,
    origin_peer_id INTEGER NOT NULL REFERENCES instances(id),
    origin_public_key TEXT NOT NULL,
    member_pseudonym TEXT NOT NULL,
    device_id TEXT NOT NULL,
    staging_path TEXT NOT NULL,
    storage_path TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK(size_bytes >= 1 AND size_bytes <= 52428828 AND (size_bytes >= 28 OR COALESCE(json_type(accepted_json, '$.descriptor.plaintext'), '') = 'object')),
    ciphertext_sha256 TEXT NOT NULL,
    committed_bytes INTEGER NOT NULL DEFAULT 0 CHECK(committed_bytes >= 0 AND committed_bytes <= size_bytes),
    prefix_sha256 TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK(status IN ('pending','receiving','ready','failed','removed')),
    attempts INTEGER NOT NULL DEFAULT 0 CHECK(attempts >= 0),
    next_retry_at INTEGER NOT NULL DEFAULT (unixepoch()),
    last_error TEXT,
    lease_token TEXT,
    lease_expires_at INTEGER,
    cleanup_pending INTEGER NOT NULL DEFAULT 0 CHECK(cleanup_pending IN (0,1)),
    notification_pending INTEGER NOT NULL DEFAULT 1 CHECK(notification_pending IN (0,1)),
    notification_seq INTEGER NOT NULL DEFAULT 1,
    removed_reason TEXT CHECK(removed_reason IN ('deleted','retention','detached','channel_deleted')),
    tombstone_seq INTEGER,
    tombstone_hash TEXT,
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);
INSERT INTO federated_upload_transfers_v2 SELECT * FROM federated_upload_transfers;
DROP TABLE federated_upload_transfers;
ALTER TABLE federated_upload_transfers_v2 RENAME TO federated_upload_transfers;
CREATE INDEX idx_federated_upload_work ON federated_upload_transfers(server_id,status,next_retry_at);
CREATE INDEX idx_federated_upload_message ON federated_upload_transfers(server_id,channel_id,message_id);
CREATE TRIGGER notify_replica_upload_status AFTER UPDATE OF status ON federated_upload_transfers
WHEN NEW.status<>OLD.status BEGIN
    UPDATE federated_upload_transfers SET notification_pending=1,notification_seq=notification_seq+1 WHERE id=NEW.id;
END;

CREATE TRIGGER cancel_deleted_replica_upload BEFORE DELETE ON uploads
WHEN OLD.purpose = 'federated_chat' BEGIN
    UPDATE federated_upload_transfers SET status='removed',
        removed_reason=COALESCE(removed_reason,'deleted'),
        lease_token=NULL,lease_expires_at=NULL,updated_at=unixepoch()
        WHERE server_id=OLD.server_id AND upload_id=OLD.upload_id;
END;
