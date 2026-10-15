-- Older federated redactions blanked only the current body. Remove their
-- historical bodies during upgrade and make every future soft delete enforce
-- the same invariant, regardless of which HTTP/WS/federation path performs it.
DELETE FROM message_edits
WHERE message_id IN (SELECT message_id FROM messages WHERE deleted_at IS NOT NULL);

CREATE TRIGGER purge_deleted_message_edit_history
AFTER UPDATE OF deleted_at ON messages WHEN NEW.deleted_at IS NOT NULL
BEGIN
    DELETE FROM message_edits WHERE message_id = NEW.message_id;
END;
