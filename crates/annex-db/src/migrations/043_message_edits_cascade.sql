-- Let a message's edit history be deleted with the message.
--
-- `message_edits.message_id` referenced `messages(message_id)` with no
-- `ON DELETE` action, and the pool sets `PRAGMA foreign_keys = ON`. The
-- retention sweep deletes a *batch* in one statement:
--
--     DELETE FROM messages WHERE rowid IN (SELECT rowid FROM messages
--       WHERE expires_at IS NOT NULL AND expires_at < datetime('now')
--       LIMIT ?1)
--
-- so the first expired message that had ever been edited raised a foreign-key
-- violation and aborted the entire statement. Not that message — the whole
-- batch, and every batch after it, because the same row is picked up again on
-- the next sweep. Message retention therefore stopped working permanently the
-- first time an edited message aged out, with no error a user or operator
-- would see: the sweep runs in a background task and logs at warn.
--
-- SQLite cannot alter a constraint in place, so the table is rebuilt. The
-- data is copied verbatim; only the foreign key changes.
--
-- No `PRAGMA foreign_keys` toggling here: migrations run inside a
-- transaction (`tx.execute_batch`) and SQLite silently IGNORES that pragma
-- inside one, so writing it would look like a safeguard while doing
-- nothing. The rebuild does not need it — `message_edits` is the child
-- side, every copied row already satisfied the constraint, and dropping a
-- child table never violates one.

CREATE TABLE message_edits_new (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id TEXT NOT NULL,
    old_content TEXT NOT NULL,
    edited_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (message_id) REFERENCES messages(message_id) ON DELETE CASCADE
);

INSERT INTO message_edits_new (id, message_id, old_content, edited_at)
    SELECT id, message_id, old_content, edited_at FROM message_edits;

DROP TABLE message_edits;
ALTER TABLE message_edits_new RENAME TO message_edits;

CREATE INDEX idx_message_edits_message_id ON message_edits(message_id);
