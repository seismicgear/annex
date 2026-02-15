CREATE TABLE agent_registrations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    pseudonym_id TEXT NOT NULL,
    alignment_status TEXT NOT NULL,    -- ALIGNED | PARTIAL | CONFLICT
    transfer_scope TEXT NOT NULL,      -- NO_TRANSFER | REFLECTION_SUMMARIES_ONLY | FULL_KNOWLEDGE_BUNDLE
    capability_contract_json TEXT NOT NULL,
    voice_profile_id INTEGER,
    reputation_score REAL NOT NULL DEFAULT 0.0,
    last_handshake_at TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    FOREIGN KEY (server_id) REFERENCES servers(id),
    UNIQUE (server_id, pseudonym_id)
);
