-- Files and SQLite cannot share a transaction. Persist the staging/cleanup
-- obligation before filesystem writes, then expose only finalized uploads.
ALTER TABLE uploads ADD COLUMN encrypted INTEGER NOT NULL DEFAULT 0 CHECK(encrypted IN (0, 1));
ALTER TABLE uploads ADD COLUMN epoch INTEGER;
ALTER TABLE uploads ADD COLUMN key_id TEXT;
ALTER TABLE uploads ADD COLUMN ciphertext_sha256 TEXT;
ALTER TABLE uploads ADD COLUMN storage_path TEXT;
ALTER TABLE uploads ADD COLUMN staging_path TEXT;
ALTER TABLE uploads ADD COLUMN lifecycle TEXT NOT NULL DEFAULT 'legacy'
    CHECK(lifecycle IN ('legacy', 'staging', 'ready'));

-- Previous filenames were generated from the verified MIME type and UUID.
UPDATE uploads SET storage_path =
    CASE WHEN purpose = 'server_image' THEN 'server/server_' ELSE 'chat/' ||
        CASE category WHEN 'image' THEN 'images/' WHEN 'video' THEN 'videos/' ELSE 'files/' END END
    || upload_id || '.' || CASE content_type
        WHEN 'image/jpeg' THEN 'jpg' WHEN 'image/png' THEN 'png'
        WHEN 'image/gif' THEN 'gif' WHEN 'image/webp' THEN 'webp'
        WHEN 'video/mp4' THEN 'mp4' WHEN 'video/webm' THEN 'webm'
        WHEN 'video/quicktime' THEN 'mov' WHEN 'application/pdf' THEN 'pdf'
        WHEN 'application/zip' THEN 'zip' WHEN 'text/plain' THEN 'txt' ELSE 'bin' END;

CREATE TABLE message_uploads (
    server_id INTEGER NOT NULL,
    message_id TEXT NOT NULL,
    upload_id TEXT NOT NULL,
    PRIMARY KEY(server_id, message_id, upload_id),
    FOREIGN KEY(message_id) REFERENCES messages(message_id) ON DELETE CASCADE,
    FOREIGN KEY(upload_id) REFERENCES uploads(upload_id) ON DELETE CASCADE
);
CREATE INDEX idx_message_uploads_upload ON message_uploads(server_id, upload_id);

-- A delayed cleanup obligation must never delete a new upload that reused the
-- old UUID. IDs remain consumed even after logical deletion or upload abort.
CREATE TABLE upload_ids_used (
    upload_id TEXT PRIMARY KEY,
    server_id INTEGER NOT NULL
);
INSERT INTO upload_ids_used(upload_id, server_id) SELECT upload_id, server_id FROM uploads;
CREATE TRIGGER reserve_upload_id AFTER INSERT ON uploads BEGIN
    INSERT INTO upload_ids_used(upload_id, server_id) VALUES (NEW.upload_id, NEW.server_id);
END;

CREATE TABLE upload_deletion_queue (
    server_id INTEGER NOT NULL,
    relative_path TEXT NOT NULL,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    queued_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(server_id, relative_path)
);

CREATE TABLE upload_migration_state (
    server_id INTEGER PRIMARY KEY,
    legacy_associations_complete INTEGER NOT NULL DEFAULT 0,
    FOREIGN KEY(server_id) REFERENCES servers(id) ON DELETE CASCADE
);

CREATE TRIGGER queue_removed_upload_files BEFORE DELETE ON uploads BEGIN
    INSERT OR IGNORE INTO upload_deletion_queue(server_id, relative_path)
        SELECT OLD.server_id, OLD.storage_path WHERE OLD.storage_path IS NOT NULL;
    INSERT OR IGNORE INTO upload_deletion_queue(server_id, relative_path)
        SELECT OLD.server_id, OLD.staging_path WHERE OLD.staging_path IS NOT NULL;
END;

CREATE TRIGGER cleanup_unreferenced_message_upload AFTER DELETE ON message_uploads BEGIN
    DELETE FROM uploads WHERE server_id = OLD.server_id AND upload_id = OLD.upload_id
        AND NOT EXISTS (SELECT 1 FROM message_uploads
            WHERE server_id = OLD.server_id AND upload_id = OLD.upload_id);
END;

CREATE TRIGGER cleanup_soft_deleted_message_uploads AFTER UPDATE OF deleted_at ON messages
WHEN NEW.deleted_at IS NOT NULL AND OLD.deleted_at IS NULL BEGIN
    DELETE FROM message_uploads WHERE server_id = NEW.server_id AND message_id = NEW.message_id;
END;

-- Independent of FK recursive-trigger behavior, retention and hard deletion
-- enter the same durable cleanup path before the message disappears.
CREATE TRIGGER cleanup_deleted_message_uploads BEFORE DELETE ON messages BEGIN
    DELETE FROM message_uploads WHERE server_id = OLD.server_id AND message_id = OLD.message_id;
END;

CREATE TRIGGER cleanup_deleted_channel_uploads BEFORE DELETE ON channels BEGIN
    DELETE FROM uploads WHERE server_id = OLD.server_id AND channel_id = OLD.channel_id;
END;
