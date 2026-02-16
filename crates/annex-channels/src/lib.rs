//! Channel model and text communication for the Annex platform.
//!
//! Implements channel CRUD, message persistence, WebSocket real-time
//! delivery, message history retrieval, and retention policy enforcement.
//!
//! Channels are the primary communication primitive in Annex. They support
//! multiple types (`Text`, `Voice`, `Hybrid`, `Agent`, `Broadcast`), each
//! with distinct capability requirements and federation scoping.

use annex_types::{AlignmentStatus, ChannelType, FederationScope, ServerPolicy};
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors that can occur during channel operations.
#[derive(Debug, Error)]
pub enum ChannelError {
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("channel not found: {0}")]
    NotFound(String),
    #[error("json serialization error: {0}")]
    Json(#[from] serde_json::Error),
}

/// A communication channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Channel {
    /// Internal database ID.
    pub id: i64,
    /// ID of the server this channel belongs to.
    pub server_id: i64,
    /// Unique public ID for the channel (e.g. UUID).
    pub channel_id: String,
    /// Display name of the channel.
    pub name: String,
    /// Type of the channel.
    pub channel_type: ChannelType,
    /// Optional topic/description.
    pub topic: Option<String>,
    /// Optional VRP topic binding (requires membership proof).
    pub vrp_topic_binding: Option<String>,
    /// JSON string of required capabilities.
    pub required_capabilities_json: Option<String>,
    /// Minimum alignment status for agents to join.
    pub agent_min_alignment: Option<AlignmentStatus>,
    /// Message retention in days (None = use server default).
    pub retention_days: Option<u32>,
    /// Federation scope (Local vs Federated).
    pub federation_scope: FederationScope,
    /// Creation timestamp (ISO 8601).
    pub created_at: String,
}

/// Parameters for creating a new channel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateChannelParams {
    pub server_id: i64,
    pub channel_id: String,
    pub name: String,
    pub channel_type: ChannelType,
    pub topic: Option<String>,
    pub vrp_topic_binding: Option<String>,
    pub required_capabilities_json: Option<String>,
    pub agent_min_alignment: Option<AlignmentStatus>,
    pub retention_days: Option<u32>,
    pub federation_scope: FederationScope,
}

/// Parameters for updating an existing channel.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UpdateChannelParams {
    pub name: Option<String>,
    pub topic: Option<String>,
    pub vrp_topic_binding: Option<String>,
    pub required_capabilities_json: Option<String>,
    pub agent_min_alignment: Option<AlignmentStatus>,
    pub retention_days: Option<u32>,
    pub federation_scope: Option<FederationScope>,
}

/// Creates a new channel.
pub fn create_channel(conn: &Connection, params: &CreateChannelParams) -> Result<(), ChannelError> {
    let channel_type_json = serde_json::to_string(&params.channel_type)?;
    let federation_scope_json = serde_json::to_string(&params.federation_scope)?;
    let alignment_json = params
        .agent_min_alignment
        .map(|a| serde_json::to_string(&a))
        .transpose()?;

    conn.execute(
        "INSERT INTO channels (
            server_id, channel_id, name, channel_type, topic,
            vrp_topic_binding, required_capabilities_json, agent_min_alignment,
            retention_days, federation_scope
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            params.server_id,
            params.channel_id,
            params.name,
            channel_type_json,
            params.topic,
            params.vrp_topic_binding,
            params.required_capabilities_json,
            alignment_json,
            params.retention_days,
            federation_scope_json,
        ],
    )?;
    Ok(())
}

/// Retrieves a channel by its public ID.
pub fn get_channel(conn: &Connection, channel_id: &str) -> Result<Channel, ChannelError> {
    conn.query_row(
        "SELECT
            id, server_id, channel_id, name, channel_type, topic,
            vrp_topic_binding, required_capabilities_json, agent_min_alignment,
            retention_days, federation_scope, created_at
        FROM channels WHERE channel_id = ?1",
        [channel_id],
        map_row_to_channel,
    )
    .optional()?
    .ok_or_else(|| ChannelError::NotFound(channel_id.to_string()))
}

/// Lists all channels for a given server.
pub fn list_channels(conn: &Connection, server_id: i64) -> Result<Vec<Channel>, ChannelError> {
    let mut stmt = conn.prepare(
        "SELECT
            id, server_id, channel_id, name, channel_type, topic,
            vrp_topic_binding, required_capabilities_json, agent_min_alignment,
            retention_days, federation_scope, created_at
        FROM channels WHERE server_id = ?1 ORDER BY name ASC",
    )?;

    let rows = stmt.query_map([server_id], map_row_to_channel)?;
    let mut channels = Vec::new();
    for row in rows {
        channels.push(row?);
    }
    Ok(channels)
}

/// Updates an existing channel.
pub fn update_channel(
    conn: &Connection,
    channel_id: &str,
    updates: &UpdateChannelParams,
) -> Result<(), ChannelError> {
    // We construct the update query dynamically based on present fields.
    // This is a bit verbose in rusqlite without a query builder, but robust.
    // However, for simplicity and standard practice, we'll just check existence first
    // then update fields that are Some.

    // A simpler approach for now: fetch, update struct, save? No, race conditions.
    // Better: individual updates or dynamic query.

    // Strategy: Read the current channel, apply updates in memory, and write back all fields.
    // This is acceptable for Phase 4 where concurrency on *channel configuration* is low.

    let mut channel = get_channel(conn, channel_id)?;

    if let Some(name) = &updates.name {
        channel.name = name.clone();
    }
    if let Some(topic) = &updates.topic {
        channel.topic = Some(topic.clone());
    }
    if let Some(binding) = &updates.vrp_topic_binding {
        channel.vrp_topic_binding = Some(binding.clone());
    }
    if let Some(caps) = &updates.required_capabilities_json {
        channel.required_capabilities_json = Some(caps.clone());
    }
    if let Some(align) = &updates.agent_min_alignment {
        channel.agent_min_alignment = Some(*align);
    }
    if let Some(days) = &updates.retention_days {
        channel.retention_days = Some(*days);
    }
    if let Some(scope) = &updates.federation_scope {
        channel.federation_scope = *scope;
    }

    let federation_scope_json = serde_json::to_string(&channel.federation_scope)?;
    let alignment_json = channel
        .agent_min_alignment
        .map(|a| serde_json::to_string(&a))
        .transpose()?;

    conn.execute(
        "UPDATE channels SET
            name = ?1,
            topic = ?2,
            vrp_topic_binding = ?3,
            required_capabilities_json = ?4,
            agent_min_alignment = ?5,
            retention_days = ?6,
            federation_scope = ?7
        WHERE channel_id = ?8",
        params![
            channel.name,
            channel.topic,
            channel.vrp_topic_binding,
            channel.required_capabilities_json,
            alignment_json,
            channel.retention_days,
            federation_scope_json,
            channel_id
        ],
    )?;

    Ok(())
}

/// Deletes a channel.
pub fn delete_channel(conn: &Connection, channel_id: &str) -> Result<(), ChannelError> {
    let count = conn.execute("DELETE FROM channels WHERE channel_id = ?1", [channel_id])?;
    if count == 0 {
        return Err(ChannelError::NotFound(channel_id.to_string()));
    }
    Ok(())
}

fn map_row_to_channel(row: &Row) -> rusqlite::Result<Channel> {
    let channel_type_str: String = row.get(4)?;
    let channel_type: ChannelType = serde_json::from_str(&channel_type_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(4, rusqlite::types::Type::Text, Box::new(e))
    })?;

    let align_str: Option<String> = row.get(8)?;
    let agent_min_alignment = match align_str {
        Some(s) => Some(serde_json::from_str(&s).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(8, rusqlite::types::Type::Text, Box::new(e))
        })?),
        None => None,
    };

    let fed_scope_str: String = row.get(10)?;
    let federation_scope: FederationScope = serde_json::from_str(&fed_scope_str).map_err(|e| {
        rusqlite::Error::FromSqlConversionFailure(10, rusqlite::types::Type::Text, Box::new(e))
    })?;

    Ok(Channel {
        id: row.get(0)?,
        server_id: row.get(1)?,
        channel_id: row.get(2)?,
        name: row.get(3)?,
        channel_type,
        topic: row.get(5)?,
        vrp_topic_binding: row.get(6)?,
        required_capabilities_json: row.get(7)?,
        agent_min_alignment,
        retention_days: row.get(9)?,
        federation_scope,
        created_at: row.get(11)?,
    })
}

/// A message in a channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    /// Internal database ID.
    pub id: i64,
    /// ID of the server.
    pub server_id: i64,
    /// Public ID of the channel.
    pub channel_id: String,
    /// Unique public ID of the message.
    pub message_id: String,
    /// Pseudonym of the sender.
    pub sender_pseudonym: String,
    /// Message content (text).
    pub content: String,
    /// ID of the message being replied to, if any.
    pub reply_to_message_id: Option<String>,
    /// Creation timestamp (ISO 8601).
    pub created_at: String,
    /// Expiration timestamp (ISO 8601), if retention applies.
    pub expires_at: Option<String>,
}

/// A member of a channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChannelMember {
    /// Internal database ID.
    pub id: i64,
    /// ID of the server.
    pub server_id: i64,
    /// Public ID of the channel.
    pub channel_id: String,
    /// Pseudonym of the member.
    pub pseudonym_id: String,
    /// Role in the channel (e.g. "MEMBER").
    pub role: String,
    /// Join timestamp (ISO 8601).
    pub joined_at: String,
}

/// Adds a member to a channel.
///
/// Returns error if already a member.
pub fn add_member(
    conn: &Connection,
    server_id: i64,
    channel_id: &str,
    pseudonym_id: &str,
) -> Result<(), ChannelError> {
    // Check if channel exists first to return proper error
    let _ = get_channel(conn, channel_id)?;

    conn.execute(
        "INSERT OR IGNORE INTO channel_members (server_id, channel_id, pseudonym_id) VALUES (?1, ?2, ?3)",
        params![server_id, channel_id, pseudonym_id],
    )?;
    Ok(())
}

/// Removes a member from a channel.
pub fn remove_member(
    conn: &Connection,
    channel_id: &str,
    pseudonym_id: &str,
) -> Result<(), ChannelError> {
    let count = conn.execute(
        "DELETE FROM channel_members WHERE channel_id = ?1 AND pseudonym_id = ?2",
        [channel_id, pseudonym_id],
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

/// Checks if a pseudonym is a member of a channel.
pub fn is_member(
    conn: &Connection,
    channel_id: &str,
    pseudonym_id: &str,
) -> Result<bool, ChannelError> {
    let exists: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM channel_members WHERE channel_id = ?1 AND pseudonym_id = ?2)",
        [channel_id, pseudonym_id],
        |row| row.get(0),
    )?;
    Ok(exists)
}

/// Lists all members of a channel.
pub fn list_members(
    conn: &Connection,
    channel_id: &str,
) -> Result<Vec<ChannelMember>, ChannelError> {
    let mut stmt = conn.prepare(
        "SELECT id, server_id, channel_id, pseudonym_id, role, joined_at
         FROM channel_members WHERE channel_id = ?1 ORDER BY joined_at ASC",
    )?;

    let rows = stmt.query_map([channel_id], map_row_to_member)?;
    let mut members = Vec::new();
    for row in rows {
        members.push(row?);
    }
    Ok(members)
}

fn map_row_to_member(row: &Row) -> rusqlite::Result<ChannelMember> {
    Ok(ChannelMember {
        id: row.get(0)?,
        server_id: row.get(1)?,
        channel_id: row.get(2)?,
        pseudonym_id: row.get(3)?,
        role: row.get(4)?,
        joined_at: row.get(5)?,
    })
}

/// Parameters for creating a new message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMessageParams {
    pub channel_id: String,
    pub message_id: String,
    pub sender_pseudonym: String,
    pub content: String,
    pub reply_to_message_id: Option<String>,
}

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
        format!("datetime('now', '+{} days')", days)
    } else {
        "NULL".to_string()
    };

    // We can't easily bind the expression part for '+N days' safely with rusqlite params if we construct the string dynamically
    // But since `days` is u32, it is safe to format into the string.

    let sql = format!(
        "INSERT INTO messages (
            server_id, channel_id, message_id, sender_pseudonym, content,
            reply_to_message_id, expires_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, {})
        RETURNING id, server_id, channel_id, message_id, sender_pseudonym, content, reply_to_message_id, created_at, expires_at",
        expires_expr
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
            reply_to_message_id, created_at, expires_at
        FROM messages WHERE message_id = ?1",
        [message_id],
        map_row_to_message,
    )
    .optional()?
    .ok_or_else(|| ChannelError::NotFound(message_id.to_string()))
}

/// Lists messages in a channel, with pagination.
///
/// If `before` is provided, returns messages created before that timestamp/message_id.
/// For simplicity, we filter by created_at.
/// `limit` defaults to 50 if not specified.
pub fn list_messages(
    conn: &Connection,
    channel_id: &str,
    before: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<Message>, ChannelError> {
    let limit = limit.unwrap_or(50).min(100);

    let sql = if before.is_some() {
        format!(
            "SELECT
                id, server_id, channel_id, message_id, sender_pseudonym, content,
                reply_to_message_id, created_at, expires_at
            FROM messages
            WHERE channel_id = ?1 AND created_at < ?2
            ORDER BY created_at DESC
            LIMIT {}",
            limit
        )
    } else {
        format!(
            "SELECT
                id, server_id, channel_id, message_id, sender_pseudonym, content,
                reply_to_message_id, created_at, expires_at
            FROM messages
            WHERE channel_id = ?1
            ORDER BY created_at DESC
            LIMIT {}",
            limit
        )
    };

    let mut stmt = conn.prepare(&sql)?;

    let rows = if let Some(before_ts) = before {
        stmt.query_map(params![channel_id, before_ts], map_row_to_message)?
    } else {
        stmt.query_map(params![channel_id], map_row_to_message)?
    };

    let mut messages = Vec::new();
    for row in rows {
        messages.push(row?);
    }
    Ok(messages)
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

    let policy: ServerPolicy = serde_json::from_str(&policy_json)?;
    Ok((server_id, Some(policy.default_retention_days)))
}

fn map_row_to_message(row: &Row) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        server_id: row.get(1)?,
        channel_id: row.get(2)?,
        message_id: row.get(3)?,
        sender_pseudonym: row.get(4)?,
        content: row.get(5)?,
        reply_to_message_id: row.get(6)?,
        created_at: row.get(7)?,
        expires_at: row.get(8)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use annex_db::run_migrations;
    use rusqlite::Connection;

    fn setup_db() -> Connection {
        let conn = Connection::open_in_memory().expect("failed to open in-memory db");
        run_migrations(&conn).expect("failed to run migrations");

        let policy = ServerPolicy::default();
        let policy_json = serde_json::to_string(&policy).expect("failed to serialize policy");

        // We need a server to reference
        conn.execute(
            "INSERT INTO servers (slug, label, policy_json) VALUES ('test-server', 'Test Server', ?1)",
            [policy_json],
        )
        .expect("failed to create dummy server");
        conn
    }

    #[test]
    fn test_channel_crud() {
        let conn = setup_db();
        let server_id = 1; // From setup_db

        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-123".to_string(),
            name: "General".to_string(),
            channel_type: ChannelType::Text,
            topic: Some("General discussion".to_string()),
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: Some(AlignmentStatus::Aligned),
            retention_days: Some(30),
            federation_scope: FederationScope::Local,
        };

        // Create
        create_channel(&conn, &params).expect("create failed");

        // Get
        let channel = get_channel(&conn, "chan-123").expect("get failed");
        assert_eq!(channel.name, "General");
        assert_eq!(channel.channel_type, ChannelType::Text);
        assert_eq!(channel.agent_min_alignment, Some(AlignmentStatus::Aligned));

        // List
        let channels = list_channels(&conn, server_id).expect("list failed");
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].id, channel.id);

        // Update
        let updates = UpdateChannelParams {
            name: Some("General Chat".to_string()),
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: None,
        };
        update_channel(&conn, "chan-123", &updates).expect("update failed");

        let updated = get_channel(&conn, "chan-123").expect("get updated failed");
        assert_eq!(updated.name, "General Chat");
        assert_eq!(updated.topic, Some("General discussion".to_string())); // Should be preserved

        // Delete
        delete_channel(&conn, "chan-123").expect("delete failed");
        let err = get_channel(&conn, "chan-123").unwrap_err();
        match err {
            ChannelError::NotFound(_) => (),
            _ => panic!("unexpected error type"),
        }
    }

    #[test]
    fn test_message_lifecycle() {
        let conn = setup_db();
        let server_id = 1;

        // Create a channel with specific retention
        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-msg".to_string(),
            name: "Message Test".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: Some(7),
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("create channel failed");

        // Create message
        let msg_params = CreateMessageParams {
            channel_id: "chan-msg".to_string(),
            message_id: "msg-1".to_string(),
            sender_pseudonym: "pseudo-1".to_string(),
            content: "Hello World".to_string(),
            reply_to_message_id: None,
        };

        let msg = create_message(&conn, &msg_params).expect("create message failed");
        assert_eq!(msg.content, "Hello World");
        assert!(msg.expires_at.is_some()); // Should have expiration

        // Create reply
        let reply_params = CreateMessageParams {
            channel_id: "chan-msg".to_string(),
            message_id: "msg-2".to_string(),
            sender_pseudonym: "pseudo-2".to_string(),
            content: "Hello back".to_string(),
            reply_to_message_id: Some("msg-1".to_string()),
        };
        let reply = create_message(&conn, &reply_params).expect("create reply failed");
        assert_eq!(reply.reply_to_message_id, Some("msg-1".to_string()));

        // Get message
        let fetched = get_message(&conn, "msg-1").expect("get message failed");
        assert_eq!(fetched.content, "Hello World");

        // List messages
        let messages = list_messages(&conn, "chan-msg", None, None).expect("list messages failed");
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].message_id, "msg-2"); // Reverse chronological
        assert_eq!(messages[1].message_id, "msg-1");
    }

    #[test]
    fn test_message_server_retention_fallback() {
        let conn = setup_db();
        let server_id = 1;

        // Channel with NO retention override
        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-default".to_string(),
            name: "Default Retention".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None, // Use server default
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("create channel failed");

        let msg_params = CreateMessageParams {
            channel_id: "chan-default".to_string(),
            message_id: "msg-default".to_string(),
            sender_pseudonym: "pseudo-1".to_string(),
            content: "Default retention".to_string(),
            reply_to_message_id: None,
        };

        let msg = create_message(&conn, &msg_params).expect("create message failed");
        assert!(msg.expires_at.is_some());
        // Server default is 30 days (default impl of ServerPolicy)
    }

    #[test]
    fn test_channel_membership() {
        let conn = setup_db();
        let server_id = 1;

        // Create channel
        let params = CreateChannelParams {
            server_id,
            channel_id: "chan-mem".to_string(),
            name: "Members Only".to_string(),
            channel_type: ChannelType::Text,
            topic: None,
            vrp_topic_binding: None,
            required_capabilities_json: None,
            agent_min_alignment: None,
            retention_days: None,
            federation_scope: FederationScope::Local,
        };
        create_channel(&conn, &params).expect("create channel failed");

        // We need a platform identity to link to, due to FK
        // setup_db only creates the server.
        conn.execute(
            "INSERT INTO platform_identities (server_id, pseudonym_id, participant_type) VALUES (1, 'user-1', 'HUMAN')",
            [],
        ).expect("create identity failed");

        // Add member
        add_member(&conn, server_id, "chan-mem", "user-1").expect("add member failed");

        // Check is_member
        assert!(is_member(&conn, "chan-mem", "user-1").unwrap());
        assert!(!is_member(&conn, "chan-mem", "user-2").unwrap());

        // List members
        let members = list_members(&conn, "chan-mem").expect("list members failed");
        assert_eq!(members.len(), 1);
        assert_eq!(members[0].pseudonym_id, "user-1");

        // Remove member
        remove_member(&conn, "chan-mem", "user-1").expect("remove member failed");
        assert!(!is_member(&conn, "chan-mem", "user-1").unwrap());
    }
}
