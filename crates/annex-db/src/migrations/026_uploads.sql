-- Uploads tracking table for images and files.
CREATE TABLE uploads (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    upload_id TEXT NOT NULL UNIQUE,
    uploader_pseudonym TEXT NOT NULL,
    original_filename TEXT NOT NULL,
    content_type TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    purpose TEXT NOT NULL DEFAULT 'chat',
    channel_id TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (server_id) REFERENCES servers(id)
);

CREATE INDEX idx_uploads_server ON uploads(server_id);
CREATE INDEX idx_uploads_channel ON uploads(channel_id);

-- Server image URL column.
ALTER TABLE servers ADD COLUMN image_url TEXT;
