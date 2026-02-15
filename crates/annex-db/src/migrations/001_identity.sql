CREATE TABLE vrp_leaves (
    leaf_index INTEGER PRIMARY KEY,
    commitment_hex TEXT NOT NULL,
    inserted_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE vrp_roots (
    root_hex TEXT NOT NULL,
    active INTEGER NOT NULL DEFAULT 1,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);
