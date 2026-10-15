-- A cancellation may arrive before multipart's first database write. Keep an
-- owner/channel-bound intent and reserve its UUID across deletion and restart.
CREATE TABLE upload_cancellations (
    server_id INTEGER NOT NULL,
    upload_id TEXT NOT NULL,
    channel_id TEXT NOT NULL,
    uploader_pseudonym TEXT NOT NULL,
    was_unpersisted INTEGER NOT NULL CHECK(was_unpersisted IN (0,1)),
    canceled_at INTEGER NOT NULL DEFAULT(unixepoch()),
    PRIMARY KEY(server_id,upload_id),
    FOREIGN KEY(server_id) REFERENCES servers(id)
);
CREATE INDEX upload_cancellations_budget
    ON upload_cancellations(server_id,was_unpersisted,canceled_at,uploader_pseudonym);
