CREATE TABLE federated_identities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    remote_instance_id INTEGER NOT NULL,
    commitment_hex TEXT NOT NULL,
    pseudonym_id TEXT NOT NULL,
    vrp_topic TEXT NOT NULL,
    attested_at TEXT NOT NULL DEFAULT (datetime('now')),
    expires_at TEXT,
    metadata_json TEXT,
    FOREIGN KEY (remote_instance_id) REFERENCES instances(id),
    FOREIGN KEY (server_id) REFERENCES servers(id),
    UNIQUE (server_id, remote_instance_id, pseudonym_id)
);
