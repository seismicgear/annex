CREATE TABLE channels (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  server_id INTEGER NOT NULL,
  channel_id TEXT NOT NULL UNIQUE,
  name TEXT NOT NULL,
  channel_type TEXT NOT NULL,         -- TEXT | VOICE | HYBRID | AGENT | BROADCAST
  topic TEXT,
  vrp_topic_binding TEXT,            -- VRP topic required for membership proof
  required_capabilities_json TEXT,   -- capability flags needed to join
  agent_min_alignment TEXT,          -- minimum VrpAlignmentStatus for agents
  retention_days INTEGER,            -- NULL = use server default
  federation_scope TEXT NOT NULL DEFAULT 'LOCAL',  -- LOCAL | FEDERATED
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  FOREIGN KEY (server_id) REFERENCES servers(id)
);
