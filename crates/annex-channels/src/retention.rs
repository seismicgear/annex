//! Retention sweep: a single batched DELETE removes up to
//! [`RETENTION_BATCH_LIMIT`] messages whose `expires_at` is in the past.
//! The caller drives the loop until `delete_expired_messages` returns less
//! than the batch limit, keeping each write transaction short so readers
//! aren't blocked.

use rusqlite::Connection;

use crate::error::ChannelError;

/// Maximum number of messages to delete in a single retention sweep.
/// Prevents long-running write transactions from blocking readers.
const RETENTION_BATCH_LIMIT: usize = 5_000;

/// Deletes messages that have passed their expiration time, up to
/// [`RETENTION_BATCH_LIMIT`] rows per call. Returns the number of rows deleted.
///
/// The caller should loop until the return value is less than the batch limit
/// to ensure all expired messages are eventually removed.
pub fn delete_expired_messages(conn: &Connection) -> Result<usize, ChannelError> {
    let count = conn.execute(
        "DELETE FROM messages WHERE rowid IN (\
             SELECT rowid FROM messages \
             WHERE expires_at IS NOT NULL AND expires_at < datetime('now') \
             LIMIT ?1\
         )",
        [RETENTION_BATCH_LIMIT],
    )?;
    Ok(count)
}
