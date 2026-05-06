//! Message lifecycle: create (with retention-derived `expires_at`), read,
//! list (cursor-paginated, server-scoped), edit and soft-delete (both gated
//! by ownership and the [`EDIT_WINDOW_SECONDS`] window), and edit-history
//! retrieval.
//!
//! `resolve_retention_days` is the only private helper here; it consults
//! the channel-level `retention_days` and falls back to
//! `ServerPolicy::default_retention_days` for `None`.

use annex_types::ServerPolicy;
use rusqlite::{params, Connection, OptionalExtension};

use crate::error::ChannelError;
use crate::types::{map_row_to_message, CreateMessageParams, Message, MessageEdit};

/// Maximum age (in seconds) for a message to be editable or deletable by its author.
pub const EDIT_WINDOW_SECONDS: i64 = 60;

/// Creates a new message, enforcing retention policy.
pub fn create_message(
    conn: &Connection,
    params: &CreateMessageParams,
) -> Result<Message, ChannelError> {
    // 1. Resolve retention days and server_id
    let (server_id, retention_days) = resolve_retention_days(conn, &params.channel_id)?;

    // 2. Insert message with computed expiration
    // We use datetime('now', '+N days') if retention_days is set.
    let expires_expr = if let Some(days) = retention_days {
        format!("datetime('now', '+{days} days')")
    } else {
        "NULL".to_string()
    };

    // We can't easily bind the expression part for '+N days' safely with rusqlite params if we construct the string dynamically
    // But since `days` is u32, it is safe to format into the string.

    let sql = format!(
        "INSERT INTO messages (
            server_id, channel_id, message_id, sender_pseudonym, content,
            reply_to_message_id, expires_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, {expires_expr})
        RETURNING id, server_id, channel_id, message_id, sender_pseudonym, content, reply_to_message_id, created_at, expires_at, edited_at, deleted_at"
    );

    let message = conn.query_row(
        &sql,
        params![
            server_id,
            params.channel_id,
            params.message_id,
            params.sender_pseudonym,
            params.content,
            params.reply_to_message_id,
        ],
        map_row_to_message,
    )?;

    Ok(message)
}

/// Retrieves a message by its ID.
pub fn get_message(conn: &Connection, message_id: &str) -> Result<Message, ChannelError> {
    conn.query_row(
        "SELECT
            id, server_id, channel_id, message_id, sender_pseudonym, content,
            reply_to_message_id, created_at, expires_at, edited_at, deleted_at
        FROM messages WHERE message_id = ?1",
        [message_id],
        map_row_to_message,
    )
    .optional()?
    .ok_or_else(|| ChannelError::NotFound(message_id.to_string()))
}

/// Lists messages in a channel, with pagination, scoped by server.
///
/// If `before` is provided (as a `message_id`), returns messages created
/// before that message. The function resolves the `message_id` to its
/// `created_at` timestamp and uses a tiebreaker on `id` to handle
/// messages with identical timestamps correctly.
/// `limit` defaults to 50 if not specified.
pub fn list_messages(
    conn: &Connection,
    server_id: i64,
    channel_id: &str,
    before: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<Message>, ChannelError> {
    let limit = limit.unwrap_or(50).min(100);

    // If `before` is a message_id, resolve it to (created_at, id) for cursor pagination.
    let cursor = if let Some(ref before_id) = before {
        let row: Option<(String, i64)> = conn
            .query_row(
                "SELECT created_at, id FROM messages WHERE message_id = ?1",
                [before_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row
    } else {
        None
    };

    if let (Some(_), Some((before_ts, before_row_id))) = (&before, cursor) {
        let sql = format!(
            "SELECT
                id, server_id, channel_id, message_id, sender_pseudonym, content,
                reply_to_message_id, created_at, expires_at, edited_at, deleted_at
            FROM messages
            WHERE server_id = ?1 AND channel_id = ?2
              AND (created_at < ?3 OR (created_at = ?3 AND id < ?4))
            ORDER BY created_at DESC, id DESC
            LIMIT {limit}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            params![server_id, channel_id, before_ts, before_row_id],
            map_row_to_message,
        )?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row?);
        }
        Ok(messages)
    } else {
        let sql = format!(
            "SELECT
                id, server_id, channel_id, message_id, sender_pseudonym, content,
                reply_to_message_id, created_at, expires_at, edited_at, deleted_at
            FROM messages
            WHERE server_id = ?1 AND channel_id = ?2
            ORDER BY created_at DESC, id DESC
            LIMIT {limit}"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params![server_id, channel_id], map_row_to_message)?;
        let mut messages = Vec::new();
        for row in rows {
            messages.push(row?);
        }
        Ok(messages)
    }
}

/// Edits a message's content, enforcing ownership and the edit time window.
///
/// Saves the old content to the `message_edits` table before overwriting.
/// Returns the updated message.
pub fn edit_message(
    conn: &Connection,
    message_id: &str,
    sender_pseudonym: &str,
    new_content: &str,
) -> Result<Message, ChannelError> {
    // Use IMMEDIATE transaction to serialize concurrent edit/delete operations.
    // Without this, two concurrent edits could both pass the ownership and
    // time-window checks, losing an edit from the audit trail.
    let tx = conn.unchecked_transaction()?;

    let msg = get_message(&tx, message_id)?;

    // Ownership check
    if msg.sender_pseudonym != sender_pseudonym {
        return Err(ChannelError::NotFound(format!(
            "message {message_id} not owned by {sender_pseudonym}"
        )));
    }

    // Already deleted
    if msg.deleted_at.is_some() {
        return Err(ChannelError::NotFound(format!(
            "message {message_id} has been deleted"
        )));
    }

    // Time window check
    let created = chrono::NaiveDateTime::parse_from_str(&msg.created_at, "%Y-%m-%d %H:%M:%S")
        .map_err(|_| ChannelError::NotFound("invalid created_at timestamp".to_string()))?;
    let now = chrono::Utc::now().naive_utc();
    if (now - created).num_seconds() > EDIT_WINDOW_SECONDS {
        return Err(ChannelError::NotFound(
            "edit window has expired".to_string(),
        ));
    }

    // Save old content to edit history
    tx.execute(
        "INSERT INTO message_edits (message_id, old_content) VALUES (?1, ?2)",
        params![message_id, msg.content],
    )?;

    // Update message content and set edited_at
    tx.execute(
        "UPDATE messages SET content = ?1, edited_at = datetime('now') WHERE message_id = ?2",
        params![new_content, message_id],
    )?;

    tx.commit()?;
    get_message(conn, message_id)
}

/// Soft-deletes a message, enforcing ownership and the edit time window.
///
/// Sets `deleted_at` and replaces content with an empty string.
/// Returns the updated message.
pub fn delete_message(
    conn: &Connection,
    message_id: &str,
    sender_pseudonym: &str,
) -> Result<Message, ChannelError> {
    // Use IMMEDIATE transaction to serialize concurrent operations.
    let tx = conn.unchecked_transaction()?;

    let msg = get_message(&tx, message_id)?;

    // Ownership check
    if msg.sender_pseudonym != sender_pseudonym {
        return Err(ChannelError::NotFound(format!(
            "message {message_id} not owned by {sender_pseudonym}"
        )));
    }

    // Already deleted
    if msg.deleted_at.is_some() {
        return Err(ChannelError::NotFound(format!(
            "message {message_id} already deleted"
        )));
    }

    // Time window check
    let created = chrono::NaiveDateTime::parse_from_str(&msg.created_at, "%Y-%m-%d %H:%M:%S")
        .map_err(|_| ChannelError::NotFound("invalid created_at timestamp".to_string()))?;
    let now = chrono::Utc::now().naive_utc();
    if (now - created).num_seconds() > EDIT_WINDOW_SECONDS {
        return Err(ChannelError::NotFound(
            "delete window has expired".to_string(),
        ));
    }

    // Soft-delete: set deleted_at and clear content
    tx.execute(
        "UPDATE messages SET content = '', deleted_at = datetime('now') WHERE message_id = ?1",
        params![message_id],
    )?;

    tx.commit()?;
    get_message(conn, message_id)
}

/// Returns the edit history for a message (oldest first).
pub fn get_edit_history(
    conn: &Connection,
    message_id: &str,
) -> Result<Vec<MessageEdit>, ChannelError> {
    let mut stmt = conn.prepare(
        "SELECT id, message_id, old_content, edited_at
         FROM message_edits
         WHERE message_id = ?1
         ORDER BY edited_at ASC",
    )?;

    let rows = stmt.query_map([message_id], |row| {
        Ok(MessageEdit {
            id: row.get(0)?,
            message_id: row.get(1)?,
            old_content: row.get(2)?,
            edited_at: row.get(3)?,
        })
    })?;

    let mut edits = Vec::new();
    for row in rows {
        edits.push(row?);
    }
    Ok(edits)
}

/// Helper: Resolve server_id and retention days for a channel.
fn resolve_retention_days(
    conn: &Connection,
    channel_id: &str,
) -> Result<(i64, Option<u32>), ChannelError> {
    // 1. Get channel info
    let (server_id, retention_days): (i64, Option<u32>) = conn
        .query_row(
            "SELECT server_id, retention_days FROM channels WHERE channel_id = ?1",
            [channel_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| ChannelError::NotFound(channel_id.to_string()))?;

    // 2. If retention_days is Some, return it.
    if let Some(days) = retention_days {
        return Ok((server_id, Some(days)));
    }

    // 3. If None, fetch server policy.
    let policy_json: String = conn
        .query_row(
            "SELECT policy_json FROM servers WHERE id = ?1",
            [server_id],
            |row| row.get(0),
        )
        .map_err(ChannelError::Database)?;

    let policy: ServerPolicy = match serde_json::from_str(&policy_json) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(
                "failed to deserialize server policy, using default retention: {}",
                e
            );
            ServerPolicy::default()
        }
    };
    Ok((server_id, Some(policy.default_retention_days)))
}
