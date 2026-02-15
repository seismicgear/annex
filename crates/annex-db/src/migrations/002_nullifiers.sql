CREATE TABLE zk_nullifiers (
    topic TEXT NOT NULL,
    nullifier_hex TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (topic, nullifier_hex)
);
