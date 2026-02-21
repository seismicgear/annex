-- Stores encrypted usernames per server member.
-- Usernames are encrypted with a server-scoped key derived from the
-- server's signing key, so they are opaque at rest and invisible to
-- federation peers or external API consumers.
CREATE TABLE user_profiles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    pseudonym_id TEXT NOT NULL,
    encrypted_username TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(server_id, pseudonym_id),
    FOREIGN KEY (server_id) REFERENCES servers(id)
);

-- Tracks explicit username visibility grants between users.
-- A grant from user A to user B means B can see A's decrypted username.
CREATE TABLE username_grants (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    granter_pseudonym TEXT NOT NULL,
    grantee_pseudonym TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE(server_id, granter_pseudonym, grantee_pseudonym),
    FOREIGN KEY (server_id) REFERENCES servers(id)
);

CREATE INDEX idx_user_profiles_server ON user_profiles(server_id, pseudonym_id);
CREATE INDEX idx_username_grants_grantee ON username_grants(server_id, grantee_pseudonym);
CREATE INDEX idx_username_grants_granter ON username_grants(server_id, granter_pseudonym);
