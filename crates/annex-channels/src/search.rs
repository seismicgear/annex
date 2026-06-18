//! Substring search across messages, scoped by server and optionally by
//! channel. Soft-deleted rows are excluded; results are capped at `limit`
//! (50 max) and ordered most-recent-first.

use rusqlite::{params, Connection};

use crate::error::ChannelError;
use crate::types::{map_row_to_message, Message};

/// Returns the most recent non-deleted messages in a channel (newest first),
/// up to `limit`, WITHOUT any content filter.
///
/// This is the scan primitive behind encrypted-at-rest search: when message
/// bodies are stored encrypted, a SQL `LIKE` cannot match, so the caller scans a
/// bounded recent window, decrypts in memory, and filters there. Keeping the SQL
/// here (and the crypto in the caller) lets `annex-channels` stay key-free.
pub fn scan_messages(
    conn: &Connection,
    server_id: i64,
    channel_id: &str,
    limit: u32,
) -> Result<Vec<Message>, ChannelError> {
    let mut stmt = conn.prepare(
        "SELECT id, server_id, channel_id, message_id, sender_pseudonym, content,
                reply_to_message_id, created_at, expires_at, edited_at, deleted_at
         FROM messages
         WHERE server_id = ?1 AND channel_id = ?2 AND deleted_at IS NULL
         ORDER BY created_at DESC, id DESC
         LIMIT ?3",
    )?;
    let rows = stmt.query_map(params![server_id, channel_id, limit], map_row_to_message)?;
    let mut messages = Vec::new();
    for row in rows {
        messages.push(row?);
    }
    Ok(messages)
}

/// Searches messages by content substring, scoped by server and optionally by channel.
///
/// Results are ordered by relevance (most recent first), capped at `limit`.
pub fn search_messages(
    conn: &Connection,
    server_id: i64,
    channel_id: Option<&str>,
    query: &str,
    limit: u32,
) -> Result<Vec<Message>, ChannelError> {
    let limit = limit.min(50);
    let pattern = format!("%{query}%");

    if let Some(cid) = channel_id {
        let mut stmt = conn.prepare(
            "SELECT id, server_id, channel_id, message_id, sender_pseudonym, content,
                    reply_to_message_id, created_at, expires_at, edited_at, deleted_at
             FROM messages
             WHERE server_id = ?1 AND channel_id = ?2 AND content LIKE ?3
               AND deleted_at IS NULL
             ORDER BY created_at DESC
             LIMIT ?4",
        )?;
        let rows = stmt.query_map(params![server_id, cid, pattern, limit], map_row_to_message)?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row?);
        }
        Ok(messages)
    } else {
        let mut stmt = conn.prepare(
            "SELECT id, server_id, channel_id, message_id, sender_pseudonym, content,
                    reply_to_message_id, created_at, expires_at, edited_at, deleted_at
             FROM messages
             WHERE server_id = ?1 AND content LIKE ?2
               AND deleted_at IS NULL
             ORDER BY created_at DESC
             LIMIT ?3",
        )?;
        let rows = stmt.query_map(params![server_id, pattern, limit], map_row_to_message)?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row?);
        }
        Ok(messages)
    }
}
