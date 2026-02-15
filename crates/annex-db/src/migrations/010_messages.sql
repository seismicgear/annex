CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    channel_id TEXT NOT NULL,
    message_id TEXT NOT NULL UNIQUE,
    sender_pseudonym TEXT NOT NULL,
    content TEXT NOT NULL,
    reply_to_message_id TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT,                    -- computed from retention policy
    FOREIGN KEY (channel_id) REFERENCES channels(channel_id),
    FOREIGN KEY (server_id) REFERENCES servers(id)
);

CREATE INDEX idx_messages_channel_created ON messages(channel_id, created_at);
