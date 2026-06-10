//! Retention sweep: a single batched DELETE removes up to
//! [`RETENTION_BATCH_LIMIT`] messages whose `expires_at` is in the past.
//! The caller drives the loop until `delete_expired_messages` returns less
//! than the batch limit, keeping each write transaction short so readers
//! aren't blocked.
//!
//! The same module owns the TTL sweep for the WS idempotency ledger
//! (`message_request_ids`, ADR-0010): [`prune_expired_request_ids`]
//! follows the identical batched-DELETE shape.

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

/// Deletes WS-idempotency ledger rows (`message_request_ids`) older than
/// `ttl_seconds`, up to [`RETENTION_BATCH_LIMIT`] rows per call. Returns
/// the number of rows deleted.
///
/// The ledger only needs to cover the client retry horizon (a
/// reconnecting client replaying an unacknowledged send). Without
/// eviction the table grows forever, and — as ADR-0010 notes — a stale
/// `client_request_id` from months ago would silently collide with a
/// fresh one, returning an ancient `message_id` instead of accepting
/// the new send.
///
/// After eviction a replayed `client_request_id` is treated as a brand
/// new send. That is the correct trade-off: clients retry over seconds
/// to minutes, and the configurable TTL (default 7 days) exceeds any
/// realistic reconnect window by orders of magnitude.
///
/// The caller should loop until the return value is less than the batch
/// limit, exactly like [`delete_expired_messages`].
pub fn prune_expired_request_ids(
    conn: &Connection,
    ttl_seconds: u64,
) -> Result<usize, ChannelError> {
    let cutoff_modifier = format!("-{ttl_seconds} seconds");
    let count = conn.execute(
        "DELETE FROM message_request_ids WHERE rowid IN (\
             SELECT rowid FROM message_request_ids \
             WHERE created_at < datetime('now', ?1) \
             LIMIT ?2\
         )",
        rusqlite::params![cutoff_modifier, RETENTION_BATCH_LIMIT],
    )?;
    Ok(count)
}
