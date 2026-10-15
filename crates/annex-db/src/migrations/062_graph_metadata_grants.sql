-- Sharing is owner-authorized and server-local. A grant does not create a
-- graph edge or expand degree-based visibility. Account deletion removes it.
CREATE TABLE graph_metadata_grants (
    server_id INTEGER NOT NULL,
    owner_pseudonym TEXT NOT NULL,
    grantee_pseudonym TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    PRIMARY KEY(server_id, owner_pseudonym, grantee_pseudonym),
    CHECK(owner_pseudonym <> grantee_pseudonym),
    FOREIGN KEY(server_id, owner_pseudonym)
        REFERENCES platform_identities(server_id, pseudonym_id) ON DELETE CASCADE,
    FOREIGN KEY(server_id, grantee_pseudonym)
        REFERENCES platform_identities(server_id, pseudonym_id) ON DELETE CASCADE
);
CREATE INDEX graph_metadata_grantee ON graph_metadata_grants(server_id, grantee_pseudonym);
