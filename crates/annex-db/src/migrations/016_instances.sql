CREATE TABLE instances (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  base_url TEXT NOT NULL UNIQUE,
  public_key TEXT NOT NULL,
  label TEXT NOT NULL,
  server_slug TEXT,
  status TEXT NOT NULL DEFAULT 'ACTIVE',
  last_seen_at TEXT,
  metadata_json TEXT,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
