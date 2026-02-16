CREATE TABLE graph_nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    pseudonym_id TEXT NOT NULL,
    node_type TEXT NOT NULL,           -- HUMAN | AI_AGENT | COLLECTIVE | BRIDGE | SERVICE
    active INTEGER NOT NULL DEFAULT 1,
    last_seen_at TEXT,
    metadata_json TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (server_id, pseudonym_id)
);
