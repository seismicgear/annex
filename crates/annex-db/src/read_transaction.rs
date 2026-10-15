//! Consistent reader snapshots that SQLite itself forbids from becoming writers.
use rusqlite::{Connection, Result, Transaction, TransactionBehavior};
use std::ops::Deref;

#[must_use]
pub struct ReadTransaction<'conn> {
    conn: &'conn Connection,
    transaction: Option<Transaction<'conn>>,
    previous_query_only: bool,
    restore_needed: bool,
}

/// The caller exclusively owns its pooled connection until this guard drops.
/// Unlike an ordinary DEFERRED transaction, an accidental write is refused by
/// SQLite instead of attempting a racy WAL snapshot upgrade.
pub fn read_transaction(conn: &Connection) -> Result<ReadTransaction<'_>> {
    let previous_query_only = conn.pragma_query_value(None, "query_only", |row| row.get(0))?;
    conn.pragma_update(None, "query_only", true)?;
    match Transaction::new_unchecked(conn, TransactionBehavior::Deferred) {
        Ok(transaction) => Ok(ReadTransaction {
            conn,
            transaction: Some(transaction),
            previous_query_only,
            restore_needed: true,
        }),
        Err(error) => {
            conn.pragma_update(None, "query_only", previous_query_only)?;
            Err(error)
        }
    }
}

impl ReadTransaction<'_> {
    fn restore(&mut self) -> Result<()> {
        if self.restore_needed {
            self.conn
                .pragma_update(None, "query_only", self.previous_query_only)?;
            self.restore_needed = false;
        }
        Ok(())
    }

    pub fn commit(mut self) -> Result<()> {
        let result = self
            .transaction
            .take()
            .expect("live read transaction")
            .commit();
        let restored = self.restore();
        result.and(restored)
    }
}

impl<'conn> Deref for ReadTransaction<'conn> {
    type Target = Transaction<'conn>;
    fn deref(&self) -> &Self::Target {
        self.transaction.as_ref().expect("live read transaction")
    }
}

impl Drop for ReadTransaction<'_> {
    fn drop(&mut self) {
        // Roll back first on error/unwind, then restore the pooled connection.
        drop(self.transaction.take());
        if let Err(error) = self.restore() {
            tracing::error!(%error, "failed to restore query-only connection policy");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE data(value INTEGER); INSERT INTO data VALUES(1);")
            .unwrap();
        conn
    }

    #[test]
    fn reads_succeed_but_accidental_writes_are_refused_and_commit_restores_connection() {
        let conn = database();
        let reader = read_transaction(&conn).unwrap();
        assert_eq!(
            reader
                .query_row("SELECT value FROM data", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        let error = reader.execute("UPDATE data SET value=2", []).unwrap_err();
        assert_eq!(
            error.sqlite_error_code(),
            Some(rusqlite::ErrorCode::ReadOnly)
        );
        reader.commit().unwrap();
        conn.execute("UPDATE data SET value=3", []).unwrap();
    }

    #[test]
    fn unwind_rolls_back_and_restores_the_connection() {
        let conn = database();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _reader = read_transaction(&conn).unwrap();
            panic!("injected reader failure");
        }));
        assert!(result.is_err());
        assert!(conn.is_autocommit());
        conn.execute("UPDATE data SET value=3", []).unwrap();
    }

    #[test]
    fn failed_nested_begin_preserves_the_existing_transaction_and_connection_policy() {
        let conn = database();
        conn.execute_batch("BEGIN IMMEDIATE").unwrap();
        assert!(read_transaction(&conn).is_err());
        assert!(!conn.is_autocommit());
        conn.execute("UPDATE data SET value=3", []).unwrap();
        conn.execute_batch("ROLLBACK").unwrap();
        conn.pragma_update(None, "query_only", true).unwrap();
        read_transaction(&conn).unwrap().commit().unwrap();
        assert_eq!(
            conn.execute("UPDATE data SET value=4", [])
                .unwrap_err()
                .sqlite_error_code(),
            Some(rusqlite::ErrorCode::ReadOnly)
        );
    }

    #[test]
    fn wal_snapshot_is_consistent_without_reserving_the_writer_lock() {
        let directory = tempfile::tempdir().unwrap();
        let filename = directory.path().join("snapshot.sqlite");
        let conn = Connection::open(&filename).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE data(value INTEGER); INSERT INTO data VALUES(1);").unwrap();
        let writer = Connection::open(&filename).unwrap();
        writer
            .busy_timeout(std::time::Duration::from_millis(20))
            .unwrap();
        let reader = read_transaction(&conn).unwrap();
        assert_eq!(
            reader
                .query_row("SELECT value FROM data", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        writer.execute("UPDATE data SET value=2", []).unwrap();
        assert_eq!(
            reader
                .query_row("SELECT value FROM data", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        reader.commit().unwrap();
        assert_eq!(
            conn.query_row("SELECT value FROM data", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}
