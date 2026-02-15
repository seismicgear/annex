CREATE TABLE servers (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  slug TEXT NOT NULL UNIQUE,
  label TEXT NOT NULL,
  policy_json TEXT NOT NULL,       -- current server policy
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
