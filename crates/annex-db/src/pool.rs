//! Connection pool creation and configuration.

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::OpenFlags;
use thiserror::Error;

/// Runtime tunables for SQLite connection behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DbRuntimeSettings {
    /// Busy timeout for SQLite connections, in milliseconds.
    pub busy_timeout_ms: u64,

    /// Maximum number of pooled SQLite connections.
    pub pool_max_size: u32,
}

impl Default for DbRuntimeSettings {
    fn default() -> Self {
        Self {
            busy_timeout_ms: 5_000,
            pool_max_size: 8,
        }
    }
}

/// A type alias for the SQLite connection pool.
pub type DbPool = Pool<SqliteConnectionManager>;

/// Errors that can occur when creating the database pool.
#[derive(Debug, Error)]
pub enum PoolError {
    /// Failed to build the connection pool.
    #[error("failed to create database connection pool: {0}")]
    PoolInit(#[from] r2d2::Error),
}

/// Creates a new SQLite connection pool with WAL mode and foreign keys enabled.
///
/// # Arguments
///
/// * `db_path` - Path to the SQLite database file. Use `:memory:` for an
///   in-memory database (useful for testing).
///
/// # Errors
///
/// Returns `PoolError::PoolInit` if the connection pool cannot be created.
pub fn create_pool(db_path: &str, settings: DbRuntimeSettings) -> Result<DbPool, PoolError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_FULL_MUTEX;

    let manager = SqliteConnectionManager::file(db_path)
        .with_flags(flags)
        .with_init(move |conn| {
            // Set WAL mode and verify it was accepted. In-memory databases
            // report "memory" which is expected and acceptable.
            let journal_mode: String =
                conn.query_row("PRAGMA journal_mode = WAL;", [], |row| row.get(0))?;
            if journal_mode != "wal" && journal_mode != "memory" {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
                    Some(format!(
                        "failed to set WAL journal mode, got: {}",
                        journal_mode
                    )),
                ));
            }
            conn.execute_batch(&format!(
                "PRAGMA foreign_keys = ON;
                 PRAGMA busy_timeout = {};",
                settings.busy_timeout_ms
            ))
        });

    let pool = Pool::builder()
        .max_size(settings.pool_max_size)
        .build(manager)?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_in_memory_pool() {
        let settings = DbRuntimeSettings {
            busy_timeout_ms: 2_500,
            pool_max_size: 3,
        };

        let pool = create_pool(":memory:", settings).expect("pool creation should succeed");
        let conn = pool.get().expect("should get a connection");

        // Verify WAL mode is active
        let mode: String = conn
            .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
            .expect("should query journal_mode");
        // In-memory databases may report "memory" instead of "wal"
        assert!(
            mode == "wal" || mode == "memory",
            "unexpected journal_mode: {mode}"
        );

        // Verify foreign keys are enabled
        let fk: i32 = conn
            .query_row("PRAGMA foreign_keys;", [], |row| row.get(0))
            .expect("should query foreign_keys");
        assert_eq!(fk, 1, "foreign keys should be enabled");

        // Verify busy timeout is configured
        let busy_timeout: i32 = conn
            .query_row("PRAGMA busy_timeout;", [], |row| row.get(0))
            .expect("should query busy_timeout");
        assert_eq!(busy_timeout, 2_500, "busy timeout should match settings");

        // Verify pool max size is configured
        assert_eq!(pool.max_size(), 3, "pool max size should match settings");
    }
}
