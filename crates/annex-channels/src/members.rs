//! Channel membership: add (idempotent on `UNIQUE(channel_id,
//! pseudonym_id)`), remove (idempotent), existence check, and listing
//! (capped at 10000).

use rusqlite::{params, Connection};

use crate::channels::get_channel;
use crate::error::ChannelError;
use crate::types::{map_row_to_member, ChannelMember};

/// Adds a member to a channel.
///
/// Idempotent: returns `Ok(())` if the member already exists (UNIQUE constraint
/// on `(channel_id, pseudonym_id)`). Propagates all other constraint violations
/// (e.g. FK violations) as errors instead of silently ignoring them.
pub fn add_member(
    conn: &Connection,
    server_id: i64,
    channel_id: &str,
    pseudonym_id: &str,
) -> Result<(), ChannelError> {
    // Check if channel exists first to return proper error
    let _ = get_channel(conn, channel_id)?;

    let result = conn.execute(
        "INSERT INTO channel_members (server_id, channel_id, pseudonym_id) VALUES (?1, ?2, ?3)",
        params![server_id, channel_id, pseudonym_id],
    );

    match result {
        Ok(_) => Ok(()),
        Err(rusqlite::Error::SqliteFailure(err, msg)) => {
            if err.code == rusqlite::ErrorCode::ConstraintViolation {
                // UNIQUE(channel_id, pseudonym_id) conflict → already a member, idempotent OK.
                // We distinguish this from FK violations by checking the extended code.
                // SQLITE_CONSTRAINT_UNIQUE = 2067, SQLITE_CONSTRAINT_PRIMARYKEY = 1555.
                let ext = err.extended_code;
                if ext == rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE
                    || ext == rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY
                {
                    return Ok(());
                }
            }
            Err(ChannelError::Database(rusqlite::Error::SqliteFailure(
                err, msg,
            )))
        }
        Err(e) => Err(ChannelError::Database(e)),
    }
}

/// Removes a member from a channel, scoped by server.
pub fn remove_member(
    conn: &Connection,
    server_id: i64,
    channel_id: &str,
    pseudonym_id: &str,
) -> Result<(), ChannelError> {
    let count = conn.execute(
        "DELETE FROM channel_members WHERE server_id = ?1 AND channel_id = ?2 AND pseudonym_id = ?3",
        params![server_id, channel_id, pseudonym_id],
    )?;
    if count == 0 {
        // Not considered an error if they weren't a member?
        // Or should we return NotFound?
        // Idempotency suggests OK, but for consistency with delete_channel, maybe verify membership first?
        // Usually leave is idempotent.
        return Ok(());
    }
    Ok(())
}

/// Checks if a pseudonym is a member of a channel, scoped by server.
pub fn is_member(
    conn: &Connection,
    server_id: i64,
    channel_id: &str,
    pseudonym_id: &str,
) -> Result<bool, ChannelError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM channel_members WHERE server_id = ?1 AND channel_id = ?2 AND pseudonym_id = ?3)",
        params![server_id, channel_id, pseudonym_id],
        |row| row.get(0),
    )?;
    Ok(exists)
}

/// Lists members of a channel (capped at 10000).
pub fn list_members(
    conn: &Connection,
    channel_id: &str,
) -> Result<Vec<ChannelMember>, ChannelError> {
    let mut stmt = conn.prepare(
        "SELECT id, server_id, channel_id, pseudonym_id, role, joined_at
         FROM channel_members WHERE channel_id = ?1 ORDER BY joined_at ASC
         LIMIT 10000",
    )?;

    let rows = stmt.query_map([channel_id], map_row_to_member)?;
    let mut members = Vec::new();
    for row in rows {
        members.push(row?);
    }
    Ok(members)
}
