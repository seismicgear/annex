//! Connection pool creation and configuration.

use r2d2::{ManageConnection, Pool};
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
pub type DbPool = Pool<DbConnectionManager>;

/// A returned connection must have neither an unfinished transaction nor a
/// reader-only policy. The upstream manager otherwise considers every returned
/// SQLite connection reusable, even after rollback or guard restoration fails.
#[derive(Debug)]
pub struct DbConnectionManager {
    inner: SqliteConnectionManager,
}

fn clean_connection(conn: &rusqlite::Connection) -> bool {
    conn.is_autocommit()
        && matches!(
            conn.pragma_query_value(None, "query_only", |row| row.get::<_, bool>(0)),
            Ok(false)
        )
}

impl ManageConnection for DbConnectionManager {
    type Connection = rusqlite::Connection;
    type Error = rusqlite::Error;

    fn connect(&self) -> Result<Self::Connection, Self::Error> {
        self.inner.connect()
    }

    fn is_valid(&self, conn: &mut Self::Connection) -> Result<(), Self::Error> {
        if !clean_connection(conn) {
            return Err(rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_MISUSE),
                Some("pooled connection retained transaction or read-only state".into()),
            ));
        }
        self.inner.is_valid(conn)
    }

    fn has_broken(&self, conn: &mut Self::Connection) -> bool {
        !clean_connection(conn)
    }
}

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
            // `busy_timeout` FIRST, before anything that can contend.
            //
            // It used to be set after the journal-mode switch, and switching
            // to WAL needs a brief exclusive lock. r2d2 opens several
            // connections at once, so their WAL switches raced each other
            // with the default timeout of zero and one lost instantly —
            // every server start logged `ERROR r2d2: database is locked`
            // before the first migration ran. r2d2 retried and the server
            // came up fine, so the only casualty was an operator reading an
            // ERROR at startup that meant nothing. A pragma that governs
            // waiting has to be in place before the first statement that
            // might wait.
            conn.execute_batch(&format!(
                "PRAGMA busy_timeout = {};",
                settings.busy_timeout_ms
            ))?;

            // Set WAL mode and verify it was accepted. In-memory databases
            // report "memory" which is expected and acceptable.
            let journal_mode: String =
                conn.query_row("PRAGMA journal_mode = WAL;", [], |row| row.get(0))?;
            if journal_mode != "wal" && journal_mode != "memory" {
                return Err(rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_ERROR),
                    Some(format!(
                        "failed to set WAL journal mode, got: {journal_mode}"
                    )),
                ));
            }
            conn.execute_batch("PRAGMA foreign_keys = ON;")
        });

    // In-memory databases (:memory:) create a separate, empty database for
    // each connection. With pool_max_size > 1, a background task could grab
    // a second connection that points to a different (empty) database,
    // causing queries to fail or return stale results. Clamping to 1
    // ensures all operations share the single in-memory database.
    let effective_max_size = if db_path == ":memory:" {
        1
    } else {
        settings.pool_max_size
    };

    let pool = Pool::builder()
        .max_size(effective_max_size)
        .build(DbConnectionManager { inner: manager })?;

    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_discards_unrestored_reader_policy_and_rolls_back_unfinished_writers() {
        let directory = tempfile::tempdir().unwrap();
        let filename = directory.path().join("reuse.sqlite");
        let pool = create_pool(
            filename.to_str().unwrap(),
            DbRuntimeSettings {
                pool_max_size: 1,
                ..Default::default()
            },
        )
        .unwrap();
        {
            let conn = pool.get().unwrap();
            conn.execute_batch("CREATE TABLE data(value INTEGER); INSERT INTO data VALUES(1);")
                .unwrap();
            // Exact persistent state after an unsuccessful read-guard restore.
            conn.pragma_update(None, "query_only", true).unwrap();
        }
        {
            let conn = pool.get_timeout(std::time::Duration::from_secs(5)).unwrap();
            assert!(clean_connection(&conn));
            conn.execute("UPDATE data SET value=2", []).unwrap();
            // Exact persistent state after a transaction fails to roll back.
            conn.execute_batch("BEGIN IMMEDIATE; UPDATE data SET value=99;")
                .unwrap();
        }
        let conn = pool.get_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(clean_connection(&conn));
        assert_eq!(
            conn.query_row("SELECT value FROM data", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
        conn.execute("UPDATE data SET value=3", []).unwrap();
    }

    #[test]
    fn checkout_validation_rejects_dirty_state_instead_of_resetting_it_silently() {
        let manager = DbConnectionManager {
            inner: SqliteConnectionManager::memory(),
        };
        let mut conn = manager.connect().unwrap();
        conn.pragma_update(None, "query_only", true).unwrap();
        assert!(manager.is_valid(&mut conn).is_err());
        assert!(manager.has_broken(&mut conn));
        conn.pragma_update(None, "query_only", false).unwrap();
        conn.execute_batch("BEGIN IMMEDIATE;").unwrap();
        assert!(manager.is_valid(&mut conn).is_err());
        assert!(manager.has_broken(&mut conn));
        conn.execute_batch("ROLLBACK;").unwrap();
        assert!(manager.is_valid(&mut conn).is_ok());
    }

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

        // In-memory databases are clamped to pool_max_size=1 because each
        // connection opens a separate empty database.
        assert_eq!(
            pool.max_size(),
            1,
            "in-memory pool should be clamped to max_size=1"
        );
    }

    #[test]
    fn file_pool_uses_configured_max_size() {
        let dir = tempfile::tempdir().expect("should create temp dir");
        let db_path = dir.path().join("test.db");
        let db_path_str = db_path.to_str().expect("valid utf-8 path");

        let settings = DbRuntimeSettings {
            busy_timeout_ms: 5_000,
            pool_max_size: 4,
        };

        let pool = create_pool(db_path_str, settings).expect("pool creation should succeed");
        assert_eq!(
            pool.max_size(),
            4,
            "file-backed pool should use configured max_size"
        );
    }
}

#[cfg(test)]
mod init_order_tests {
    use super::*;

    /// A pool that opens several connections at once must not race itself.
    ///
    /// `PRAGMA journal_mode = WAL` needs a brief exclusive lock, and it used
    /// to run before `PRAGMA busy_timeout` was set — so with the default
    /// timeout of zero, one of the concurrently-initialising connections lost
    /// instantly. Every server start logged `ERROR r2d2: database is locked`.
    /// The pool recovered by retrying, which is why it went unnoticed: the
    /// only casualty was an operator reading an ERROR that meant nothing.
    #[test]
    fn concurrent_connection_init_does_not_trip_the_lock() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("init-race.sqlite");
        let pool = create_pool(
            db_path.to_str().expect("utf-8 path"),
            DbRuntimeSettings {
                busy_timeout_ms: 5_000,
                pool_max_size: 8,
            },
        )
        .expect("pool builds");

        // Force every connection in the pool to be opened and initialised at
        // the same time, which is what r2d2 does at startup.
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let p = pool.clone();
                std::thread::spawn(move || {
                    let conn = p.get().expect("connection initialises");
                    let mode: String = conn
                        .query_row("PRAGMA journal_mode;", [], |r| r.get(0))
                        .expect("journal mode readable");
                    let busy: i32 = conn
                        .query_row("PRAGMA busy_timeout;", [], |r| r.get(0))
                        .expect("busy timeout readable");
                    (mode, busy)
                })
            })
            .collect();

        for h in handles {
            let (mode, busy) = h.join().expect("no panic during init");
            assert_eq!(mode, "wal", "every connection must reach WAL mode");
            assert_eq!(busy, 5_000, "every connection must carry the busy timeout");
        }
    }
}
