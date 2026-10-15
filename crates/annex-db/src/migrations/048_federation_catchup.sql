-- Keep the durable delivery sequence after removing message bodies. The sequence
-- is signed with each catch-up batch; IDs are monotonic, not necessarily dense.
ALTER TABLE federation_outbox ADD COLUMN source_message_id TEXT;
ALTER TABLE federation_outbox ADD COLUMN channel_id TEXT;
ALTER TABLE federation_outbox ADD COLUMN removed_reason TEXT;
CREATE INDEX idx_federation_outbox_catchup
    ON federation_outbox(peer_instance_id, channel_id, id);
CREATE INDEX idx_federation_outbox_source ON federation_outbox(source_message_id);

CREATE TABLE federation_catchup_cursors (
    local_server_id INTEGER NOT NULL REFERENCES servers(id),
    remote_instance_id INTEGER NOT NULL REFERENCES instances(id),
    channel_id TEXT NOT NULL,
    after_seq INTEGER NOT NULL DEFAULT 0 CHECK(after_seq >= 0),
    pruned_through_seq INTEGER,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(local_server_id, remote_instance_id, channel_id)
);
CREATE TABLE federation_catchup_tombstones (
    remote_instance_id INTEGER NOT NULL REFERENCES instances(id),
    message_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    reason TEXT NOT NULL CHECK(reason IN ('deleted', 'retention')),
    PRIMARY KEY(remote_instance_id, message_id)
);

-- A soft delete or retention sweep must remove ALL durable copies of the body,
-- including pending/failed envelopes and historical edits. Keep only routing
-- metadata so an offline peer can learn the deletion without recovering content.
CREATE TRIGGER federation_outbox_soft_delete
AFTER UPDATE OF deleted_at ON messages WHEN NEW.deleted_at IS NOT NULL
BEGIN
    INSERT OR IGNORE INTO federation_outbox
        (peer_instance_id, message_id, source_message_id, channel_id, envelope_json, removed_reason, status)
    SELECT DISTINCT peer_instance_id, 'catchup:deleted:' || NEW.message_id,
        NEW.message_id, NEW.channel_id, '', 'deleted', 'delivered'
    FROM federation_outbox WHERE source_message_id = NEW.message_id;
    UPDATE federation_outbox SET envelope_json = '', removed_reason = 'deleted',
        status = 'delivered', updated_at = datetime('now')
    WHERE source_message_id = NEW.message_id;
END;
CREATE TRIGGER federation_outbox_retention_delete
BEFORE DELETE ON messages
BEGIN
    INSERT OR IGNORE INTO federation_outbox
        (peer_instance_id, message_id, source_message_id, channel_id, envelope_json, removed_reason, status)
    SELECT DISTINCT peer_instance_id, 'catchup:retention:' || OLD.message_id,
        OLD.message_id, OLD.channel_id, '',
        CASE WHEN OLD.deleted_at IS NOT NULL THEN 'deleted' ELSE 'retention' END, 'delivered'
    FROM federation_outbox WHERE source_message_id = OLD.message_id;
    UPDATE federation_outbox SET envelope_json = '',
        removed_reason = CASE WHEN OLD.deleted_at IS NOT NULL THEN 'deleted' ELSE 'retention' END,
        status = 'delivered', updated_at = datetime('now')
    WHERE source_message_id = OLD.message_id;
END;
