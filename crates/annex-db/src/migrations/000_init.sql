-- Initial migration: proves the migration system works.
-- No tables are created in this migration. Subsequent phases
-- will add the identity, channel, graph, federation, and
-- observability tables as they are implemented.

-- Migration tracking table (used by the migration runner itself).
CREATE TABLE IF NOT EXISTS _annex_migrations (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE,
    applied_at TEXT NOT NULL DEFAULT (datetime('now'))
);
