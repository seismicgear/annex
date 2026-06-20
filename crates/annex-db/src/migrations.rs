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
    Migration {
        name: "015_voice_profiles",
        sql: include_str!("migrations/015_voice_profiles.sql"),
    },
    Migration {
        name: "016_instances",
        sql: include_str!("migrations/016_instances.sql"),
    },
    Migration {
        name: "017_federation_agreements",
        sql: include_str!("migrations/017_federation_agreements.sql"),
    },
    Migration {
        name: "018_federated_identities",
        sql: include_str!("migrations/018_federated_identities.sql"),
    },
    Migration {
        name: "019_add_remote_handshake_to_federation_agreements",
        sql: include_str!("migrations/019_add_remote_handshake_to_federation_agreements.sql"),
    },
    Migration {
        name: "020_rtx_tables",
        sql: include_str!("migrations/020_rtx_tables.sql"),
    },
    Migration {
        name: "021_rtx_provenance",
        sql: include_str!("migrations/021_rtx_provenance.sql"),
    },
    Migration {
        name: "022_public_event_log",
        sql: include_str!("migrations/022_public_event_log.sql"),
    },
    Migration {
        name: "023_production_indexes",
        sql: include_str!("migrations/023_production_indexes.sql"),
    },
    Migration {
        name: "024_nullifier_lookup_columns",
        sql: include_str!("migrations/024_nullifier_lookup_columns.sql"),
    },
    Migration {
        name: "025_promote_founder",
        sql: include_str!("migrations/025_promote_founder.sql"),
    },
    Migration {
        name: "026_uploads",
        sql: include_str!("migrations/026_uploads.sql"),
    },
    Migration {
        name: "027_upload_category",
        sql: include_str!("migrations/027_upload_category.sql"),
    },
    Migration {
        name: "028_usernames",
        sql: include_str!("migrations/028_usernames.sql"),
    },
    Migration {
        name: "029_message_edits",
        sql: include_str!("migrations/029_message_edits.sql"),
    },
    Migration {
        name: "030_federated_identity_verification",
        sql: include_str!("migrations/030_federated_identity_verification.sql"),
    },
    Migration {
        name: "031_invite_codes",
        sql: include_str!("migrations/031_invite_codes.sql"),
    },
    Migration {
        name: "032_server_description",
        sql: include_str!("migrations/032_server_description.sql"),
    },
    Migration {
        name: "033_server_public_url",
        sql: include_str!("migrations/033_server_public_url.sql"),
    },
    Migration {
        name: "034_merkle_nodes",
        sql: include_str!("migrations/034_merkle_nodes.sql"),
    },
    Migration {
        name: "035_ws_request_idempotency",
        sql: include_str!("migrations/035_ws_request_idempotency.sql"),
    },
    Migration {
        name: "036_federation_receipts",
        sql: include_str!("migrations/036_federation_receipts.sql"),
    },
    Migration {
        name: "037_federation_outbox",
        sql: include_str!("migrations/037_federation_outbox.sql"),
    },
    Migration {
        name: "038_event_log_hash_chain",
        sql: include_str!("migrations/038_event_log_hash_chain.sql"),
    },
    Migration {
        name: "039_migration_checksums",
        sql: include_str!("migrations/039_migration_checksums.sql"),
    },
    Migration {
        name: "040_message_request_ids_created_idx",
        sql: include_str!("migrations/040_message_request_ids_created_idx.sql"),
    },
    Migration {
        name: "041_e2e_channel_keys",
        sql: include_str!("migrations/041_e2e_channel_keys.sql"),
    },
    Migration {
        name: "042_agent_signing_pubkey",
        sql: include_str!("migrations/042_agent_signing_pubkey.sql"),
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

    /// The embedded migration list contains two entries with the same
    /// numeric ordinal (e.g. two `037_*` files). This catches a class
    /// of bug where a contributor's branch and another contributor's
    /// branch both created a "037" migration; only one can ever apply
    /// and the other would silently never run.
    #[error("duplicate migration ordinal {ordinal} (names: {first} and {second})")]
    DuplicateOrdinal {
        ordinal: i64,
        first: String,
        second: String,
    },

    /// An already-applied migration's recorded SHA-256 does not match
    /// the embedded SQL. Indicates that somebody edited a committed
    /// migration after it was applied — explicitly forbidden by
    /// invariant I-DB-1.
    #[error(
        "migration '{name}' has been edited after being applied (db sha256={db_sha256}, embedded sha256={embedded_sha256})"
    )]
    ChecksumMismatch {
        name: String,
        db_sha256: String,
        embedded_sha256: String,
    },
}

/// Lowercase hex SHA-256 of a migration's SQL bytes. Used by the
/// integrity check.
fn migration_sha256(sql: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(sql.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Parse the leading `NNN` ordinal off a migration name like
/// `034_merkle_nodes`. Returns `None` for non-conforming names
/// (the bootstrap row is excluded from ordinal checking).
fn parse_ordinal(name: &str) -> Option<i64> {
    let n = name.split('_').next()?;
    n.parse::<i64>().ok()
}

/// True if `table.column` exists. Used by the integrity check to
/// detect whether migration 039 has run yet (its ALTER TABLE adds
/// `sha256_hex` and `ordinal` to `_annex_migrations`).
fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool, rusqlite::Error> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
    for r in rows {
        if r? == column {
            return Ok(true);
        }
    }
    Ok(false)
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
    // 0. Duplicate-ordinal scan. Catches the "two contributors both
    //    made a 037" mistake at boot, before any migration runs.
    let mut seen: std::collections::HashMap<i64, &str> = std::collections::HashMap::new();
    for m in migrations {
        if let Some(ord) = parse_ordinal(m.name) {
            if let Some(prev) = seen.insert(ord, m.name) {
                return Err(MigrationError::DuplicateOrdinal {
                    ordinal: ord,
                    first: prev.to_string(),
                    second: m.name.to_string(),
                });
            }
        }
    }

    // 1. Ensure the tracking table exists. Bootstrap matches the
    //    pre-039 shape (name + applied_at); migration 039 adds
    //    `sha256_hex` and `ordinal` via ALTER TABLE.
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

    // 2. Integrity check on already-applied migrations — only runs
    //    after migration 039 has added the `sha256_hex` column.
    //    On a fresh DB the column does not exist yet; we skip the
    //    check (there are no applied rows to verify anyway) and the
    //    apply loop below will run 039, after which subsequent boots
    //    perform the check.
    if column_exists(conn, "_annex_migrations", "sha256_hex").map_err(MigrationError::StateQuery)? {
        for m in migrations {
            let embedded_hex = migration_sha256(m.sql);
            let recorded: Option<Option<String>> = conn
                .query_row(
                    "SELECT sha256_hex FROM _annex_migrations WHERE name = ?1",
                    [m.name],
                    |row| row.get::<_, Option<String>>(0).map(Some),
                )
                .or_else(|e| match e {
                    rusqlite::Error::QueryReturnedNoRows => Ok(None),
                    other => Err(other),
                })
                .map_err(MigrationError::StateQuery)?;

            match recorded {
                None => {} // not yet applied — handled below
                Some(None) => {
                    // Backfill rows written before 039.
                    conn.execute(
                        "UPDATE _annex_migrations SET sha256_hex = ?1, ordinal = ?2 \
                         WHERE name = ?3",
                        rusqlite::params![&embedded_hex, parse_ordinal(m.name), m.name],
                    )
                    .map_err(|e| MigrationError::ExecutionFailed {
                        name: m.name.to_string(),
                        source: e,
                    })?;
                }
                Some(Some(db_hex)) if db_hex != embedded_hex => {
                    return Err(MigrationError::ChecksumMismatch {
                        name: m.name.to_string(),
                        db_sha256: db_hex,
                        embedded_sha256: embedded_hex,
                    });
                }
                Some(Some(_)) => {} // match — good
            }
        }
    }

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

        // Record. Use the v2 shape when the columns exist; otherwise
        // fall back to the pre-039 shape so applying 039 itself
        // (which adds those columns) is not a chicken-and-egg.
        let has_columns = column_exists(&tx, "_annex_migrations", "sha256_hex").map_err(|e| {
            MigrationError::ExecutionFailed {
                name: migration.name.to_string(),
                source: e,
            }
        })?;
        if has_columns {
            let embedded_hex = migration_sha256(migration.sql);
            let ordinal = parse_ordinal(migration.name);
            tx.execute(
                "INSERT INTO _annex_migrations (name, sha256_hex, ordinal) VALUES (?1, ?2, ?3)",
                rusqlite::params![migration.name, &embedded_hex, ordinal],
            )
            .map_err(|e| MigrationError::ExecutionFailed {
                name: migration.name.to_string(),
                source: e,
            })?;
        } else {
            tx.execute(
                "INSERT INTO _annex_migrations (name) VALUES (?1)",
                rusqlite::params![migration.name],
            )
            .map_err(|e| MigrationError::ExecutionFailed {
                name: migration.name.to_string(),
                source: e,
            })?;
        }

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
        assert_eq!(applied, MIGRATIONS.len(), "should apply all migrations");

        // Verify tracking table exists and has a record
        let count: i32 = conn
            .query_row("SELECT COUNT(*) FROM _annex_migrations", [], |row| {
                row.get(0)
            })
            .expect("should query migration count");
        assert_eq!(count as usize, MIGRATIONS.len());
    }

    #[test]
    fn run_migrations_idempotent() {
        let conn = Connection::open_in_memory().expect("should open in-memory db");

        let first = run_migrations(&conn).expect("first run should succeed");
        assert_eq!(first, MIGRATIONS.len());

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

    // ── Migration integrity (introduced by migration 039) ──────────────

    #[test]
    fn parse_ordinal_extracts_leading_number() {
        assert_eq!(parse_ordinal("000_init"), Some(0));
        assert_eq!(parse_ordinal("034_merkle_nodes"), Some(34));
        assert_eq!(parse_ordinal("bootstrap"), None);
    }

    #[test]
    fn duplicate_ordinal_in_embedded_list_is_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        let bad = &[
            Migration {
                name: "010_alpha",
                sql: "CREATE TABLE a(id INTEGER);",
            },
            Migration {
                name: "010_beta",
                sql: "CREATE TABLE b(id INTEGER);",
            },
        ];
        let err = run_migrations_from_list(&conn, bad).expect_err("must reject");
        assert!(matches!(
            err,
            MigrationError::DuplicateOrdinal { ordinal: 10, .. }
        ));
    }

    /// Minimal migration set that also adds the integrity-tracking
    /// columns so the checksum check actually fires. In production
    /// this is migration 039; in unit tests we synthesise an
    /// earlier `000_integrity_columns` step so the columns exist
    /// before any other migration is applied — keeping rows from
    /// landing with NULL `sha256_hex`, which the runner would
    /// otherwise backfill against the (possibly edited) embedded SQL
    /// on a later boot and mask the very mismatch we are testing for.
    fn migs_with_integrity(target: Migration) -> Vec<Migration> {
        vec![
            Migration {
                name: "000_integrity_columns",
                sql: "ALTER TABLE _annex_migrations ADD COLUMN sha256_hex TEXT; \
                      ALTER TABLE _annex_migrations ADD COLUMN ordinal INTEGER;",
            },
            target,
        ]
    }

    #[test]
    fn checksum_mismatch_after_edit_is_rejected() {
        let conn = Connection::open_in_memory().unwrap();
        let original = migs_with_integrity(Migration {
            name: "001_create_t",
            sql: "CREATE TABLE t(x INTEGER);",
        });
        run_migrations_from_list(&conn, &original).expect("first apply");

        // Simulate a contributor editing the committed migration.
        let edited = migs_with_integrity(Migration {
            name: "001_create_t",
            sql: "CREATE TABLE t(x INTEGER, y TEXT);", // edited!
        });
        let err = run_migrations_from_list(&conn, &edited)
            .expect_err("edited migration must be rejected");
        match err {
            MigrationError::ChecksumMismatch { name, .. } => {
                assert_eq!(name, "001_create_t");
            }
            other => panic!("expected ChecksumMismatch, got {other:?}"),
        }
    }

    #[test]
    fn unchanged_already_applied_migration_passes_integrity_check() {
        let conn = Connection::open_in_memory().unwrap();
        let migs = migs_with_integrity(Migration {
            name: "001_create_t",
            sql: "CREATE TABLE t(x INTEGER);",
        });
        assert_eq!(run_migrations_from_list(&conn, &migs).unwrap(), migs.len());
        // Re-running the same set is a no-op and must not error.
        assert_eq!(run_migrations_from_list(&conn, &migs).unwrap(), 0);
    }
}
