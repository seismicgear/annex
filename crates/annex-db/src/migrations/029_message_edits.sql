-- Add edit/delete support for messages.
-- edited_at: timestamp of last edit (NULL = never edited).
-- deleted_at: timestamp of soft delete (NULL = not deleted).
ALTER TABLE messages ADD COLUMN edited_at TEXT;
ALTER TABLE messages ADD COLUMN deleted_at TEXT;

-- Edit history table: stores every previous version of a message.
CREATE TABLE message_edits (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id TEXT NOT NULL,
    old_content TEXT NOT NULL,
    edited_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (message_id) REFERENCES messages(message_id)
);

CREATE INDEX idx_message_edits_message_id ON message_edits(message_id);
