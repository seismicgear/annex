CREATE TABLE vrp_handshake_log (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    peer_pseudonym TEXT NOT NULL,
    peer_type TEXT NOT NULL, -- AGENT | SERVER
    alignment_status TEXT NOT NULL, -- ALIGNED | PARTIAL | CONFLICT
    report_json TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (server_id) REFERENCES servers(id)
);
