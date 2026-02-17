CREATE TABLE federation_agreements (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  local_server_id INTEGER NOT NULL,
  remote_instance_id INTEGER NOT NULL,
  alignment_status TEXT NOT NULL,
  transfer_scope TEXT NOT NULL,
  agreement_json TEXT NOT NULL,
  active INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  updated_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY (remote_instance_id) REFERENCES instances(id)
);
