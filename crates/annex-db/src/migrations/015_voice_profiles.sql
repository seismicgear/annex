CREATE TABLE voice_profiles (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    server_id INTEGER NOT NULL,
    profile_id TEXT NOT NULL,
    name TEXT NOT NULL,
    model TEXT NOT NULL, -- piper | bark | system
    model_path TEXT NOT NULL,
    config_path TEXT,
    speed REAL NOT NULL DEFAULT 1.0,
    pitch REAL NOT NULL DEFAULT 1.0,
    speaker_id INTEGER,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at TEXT NOT NULL DEFAULT (datetime('now')),
    UNIQUE (server_id, profile_id),
    FOREIGN KEY (server_id) REFERENCES servers(id)
);
