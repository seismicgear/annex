CREATE TABLE server_policy_versions (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  server_id INTEGER NOT NULL,
  version_id TEXT NOT NULL UNIQUE,
  policy_json TEXT NOT NULL,
  activated_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY (server_id) REFERENCES servers(id)
);
