CREATE TABLE channel_members (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  server_id INTEGER NOT NULL,
  channel_id TEXT NOT NULL,
  pseudonym_id TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'MEMBER',
  joined_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE(channel_id, pseudonym_id),
  FOREIGN KEY (server_id) REFERENCES servers(id),
  FOREIGN KEY (channel_id) REFERENCES channels(channel_id),
  FOREIGN KEY (server_id, pseudonym_id) REFERENCES platform_identities(server_id, pseudonym_id)
);
