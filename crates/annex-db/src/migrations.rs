//! Embedded SQL migration runner.
//!
//! Migrations are SQL files embedded at compile time. They run sequentially
//! on startup, tracked by the `_annex_migrations` table. Each migration
//! runs exactly once — if it has already been applied, it is skipped.

use rusqlite::Connection;
use thiserror::Error;

/// A single embedded migration.
struct Migration {
    name: &'static str,
    sql: &'static str,
}

/// All migrations in order. New migrations are appended here.
const MIGRATIONS: &[Migration] = &[
    Migration {
        name: "000_init",
        sql: include_str!("migrations/000_init.sql"),
    },
    Migration {
        name: "001_identity",
        sql: include_str!("migrations/001_identity.sql"),
    },
    Migration {
        name: "002_nullifiers",
        sql: include_str!("migrations/002_nullifiers.sql"),
    },
    Migration {
        name: "003_vrp_registry",
        sql: include_str!("migrations/003_vrp_registry.sql"),
    },
    Migration {
        name: "004_platform_identity",
        sql: include_str!("migrations/004_platform_identity.sql"),
    },
    Migration {
        name: "005_servers",
        sql: include_str!("migrations/005_servers.sql"),
    },
    Migration {
        name: "006_server_policy_versions",
        sql: include_str!("migrations/006_server_policy_versions.sql"),
    },
    Migration {
        name: "007_vrp_handshake_log",
        sql: include_str!("migrations/007_vrp_handshake_log.sql"),
    },
    Migration {
        name: "008_agent_registrations",
        sql: include_str!("migrations/008_agent_registrations.sql"),
    },
    Migration {
        name: "009_channels",
        sql: include_str!("migrations/009_channels.sql"),
    },
    Migration {
        name: "010_messages",
        sql: include_str!("migrations/010_messages.sql"),
    },
    Migration {
        name: "011_channel_members",
        sql: include_str!("migrations/011_channel_members.sql"),
    },
    Migration {
        name: "012_graph_nodes",
        sql: include_str!("migrations/012_graph_nodes.sql"),
    },
    Migration {
        name: "013_graph_edges",
        sql: include_str!("migrations/013_graph_edges.sql"),
    },
    Migration {
        name: "014_add_anchor_to_agent_registrations",
        sql: include_str!("migrations/014_add_anchor_to_agent_registrations.sql"),
    },
];

/// Errors that can occur during migration execution.
#[derive(Debug, Error)]
pub enum MigrationError {
    /// A SQL statement within a migration failed.
    #[error("migration '{name}' failed: {source}")]
    ExecutionFailed {
        /// The name of the migration that failed.
        name: String,
        /// The underlying SQLite error.
        source: rusqlite::Error,
    },

    /// Failed to query migration state.
    #[error("failed to check migration state: {0}")]
    StateQuery(rusqlite::Error),
}

/// Runs all pending migrations against the given connection.
///
/// Migrations that have already been applied (tracked in `_annex_migrations`)
/// are skipped. New migrations are applied in order and recorded.
///
/// # Errors
///
/// Returns `MigrationError` if any migration fails to execute or if the
/// migration tracking table cannot be queried.
pub fn run_migrations(conn: &Connection) -> Result<usize, MigrationError> {
    run_migrations_from_list(conn, MIGRATIONS)
}

fn run_migrations_from_list(
    conn: &Connection,
    migrations: &[Migration],
) -> Result<usize, MigrationError> {
    // Ensure the tracking table exists (the first migration creates it,
    // but we need it to exist before we can check what's been applied).
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _annex_migrations (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            name TEXT NOT NULL UNIQUE,
            applied_at TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )
    .map_err(|e| MigrationError::ExecutionFailed {
        name: "_annex_migrations_bootstrap".to_string(),
        source: e,
    })?;

    let mut applied = 0;

    for migration in migrations {
        let already_applied: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM _annex_migrations WHERE name = ?1",
                [migration.name],
                |row| row.get(0),
            )
            .map_err(MigrationError::StateQuery)?;

        if already_applied {
            tracing::debug!(
                migration = migration.name,
                "migration already applied, skipping"
            );
            continue;
        }

        tracing::info!(migration = migration.name, "applying migration");

        let tx = conn
            .unchecked_transaction()
            .map_err(|e| MigrationError::ExecutionFailed {
                name: migration.name.to_string(),
                source: e,
            })?;

        tx.execute_batch(migration.sql)
            .map_err(|e| MigrationError::ExecutionFailed {
                name: migration.name.to_string(),
                source: e,
            })?;

        tx.execute(
            "INSERT INTO _annex_migrations (name) VALUES (?1)",
            [migration.name],
        )
        .map_err(|e| MigrationError::ExecutionFailed {
            name: migration.name.to_string(),
            source: e,
        })?;

        tx.commit().map_err(|e| MigrationError::ExecutionFailed {
            name: migration.name.to_string(),
            source: e,
        })?;

        applied += 1;
    }

    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    #[test]
    fn run_migrations_on_fresh_db() {
        let conn = Connection::open_in_memory().expect("should open in-memory db");
        let applied = run_migrations(&conn).expect("migrations should succeed");
        assert_eq!(applied, 14, "should apply the initial migration");

        // Verify tracking table exists and has a record
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM _annex_migrations", [], |row| {
                row.get(0)
            })
            .expect("should query migration count");
        assert_eq!(count, 14);
    }

    #[test]
    fn run_migrations_idempotent() {
        let conn = Connection::open_in_memory().expect("should open in-memory db");

        let first = run_migrations(&conn).expect("first run should succeed");
        assert_eq!(first, 14);

        let second = run_migrations(&conn).expect("second run should succeed");
        assert_eq!(second, 0, "no new migrations to apply");
    }

    #[test]
    fn verify_vrp_registry_seeds() {
        let conn = Connection::open_in_memory().expect("should open in-memory db");
        run_migrations(&conn).expect("migrations should succeed");

        let role_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM vrp_roles", [], |row| row.get(0))
            .expect("should query vrp_roles count");
        assert_eq!(role_count, 5);

        let human_label: String = conn
            .query_row(
                "SELECT label FROM vrp_roles WHERE role_code = 1",
                [],
                |row| row.get(0),
            )
            .expect("should query human role");
        assert_eq!(human_label, "HUMAN");

        let topic_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM vrp_topics", [], |row| row.get(0))
            .expect("should query vrp_topics count");
        assert_eq!(topic_count, 3);
    }

    #[test]
    fn migration_side_effects_rollback_when_tracking_insert_fails() {
        let conn = Connection::open_in_memory().expect("should open in-memory db");
        let migrations = [Migration {
            name: "001_tracking_insert_conflict",
            sql: "
                CREATE TABLE rollback_probe (id INTEGER PRIMARY KEY);
                INSERT INTO _annex_migrations (name) VALUES ('001_tracking_insert_conflict');
            ",
        }];

        let err = run_migrations_from_list(&conn, &migrations)
            .expect_err("tracking insert conflict should fail migration");

        match err {
            MigrationError::ExecutionFailed { name, .. } => {
                assert_eq!(name, "001_tracking_insert_conflict")
            }
            other => panic!("unexpected error type: {other:?}"),
        }

        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'rollback_probe')",
                [],
                |row| row.get(0),
            )
            .expect("should query sqlite_master");

        assert!(
            !exists,
            "schema side effects should be rolled back when tracking insert fails"
        );
    }

    #[test]
    fn test_server_migrations() {
        let conn = Connection::open_in_memory().expect("should open in-memory db");
        run_migrations(&conn).expect("migrations should succeed");

        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'servers')",
                [],
                |row| row.get(0),
            )
            .expect("should query sqlite_master");
        assert!(exists, "servers table should exist");

        let exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'server_policy_versions')",
                [],
                |row| row.get(0),
            )
            .expect("should query sqlite_master");
        assert!(exists, "server_policy_versions table should exist");
    }
}
