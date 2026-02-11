//! Connection pool creation and configuration.

use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::OpenFlags;
use thiserror::Error;

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
pub fn create_pool(db_path: &str) -> Result<DbPool, PoolError> {
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE
        | OpenFlags::SQLITE_OPEN_CREATE
        | OpenFlags::SQLITE_OPEN_FULL_MUTEX;

    let manager = SqliteConnectionManager::file(db_path)
        .with_flags(flags)
        .with_init(|conn| {
            conn.execute_batch(
                "PRAGMA journal_mode = WAL;
             PRAGMA foreign_keys = ON;
             PRAGMA busy_timeout = 5000;",
            )
        });

    let pool = Pool::builder().max_size(8).build(manager)?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_in_memory_pool() {
        let pool = create_pool(":memory:").expect("pool creation should succeed");
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
    }
}
