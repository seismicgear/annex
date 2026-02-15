CREATE TABLE platform_identities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    pseudonym_id TEXT NOT NULL,
    participant_type TEXT NOT NULL,  -- HUMAN | AI_AGENT | COLLECTIVE | BRIDGE | SERVICE
    can_voice INTEGER NOT NULL DEFAULT 0,
    can_moderate INTEGER NOT NULL DEFAULT 0,
    can_invite INTEGER NOT NULL DEFAULT 0,
    can_federate INTEGER NOT NULL DEFAULT 0,
    can_bridge INTEGER NOT NULL DEFAULT 0,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (server_id, pseudonym_id)
);
