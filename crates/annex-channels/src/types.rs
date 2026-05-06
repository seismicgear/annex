//! Wire / persistence types for channels, channel members, and messages, plus
//! the `rusqlite::Row` mappers that hydrate them from query results.
//!
//! Mappers live alongside their structs (rather than inside each domain
//! module) so the JSON column conventions — `channel_type`,
//! `agent_min_alignment`, `federation_scope` — only have to be written once
//! and stay in lock-step with the struct shapes.

use annex_types::{AlignmentStatus, ChannelType, FederationScope};
use rusqlite::Row;
use serde::{Deserialize, Serialize};

/// A communication channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Channel {
    /// Internal database ID.
    #[serde(skip_serializing, default)]
    pub id: i64,
    /// ID of the server this channel belongs to.
    #[serde(skip_serializing, default)]
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

/// A message in a channel.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    /// Internal database ID.
    #[serde(skip_serializing, default)]
    pub id: i64,
    /// ID of the server.
    #[serde(skip_serializing, default)]
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
    #[serde(skip_serializing, default)]
    pub expires_at: Option<String>,
    /// Timestamp of last edit (ISO 8601), if ever edited.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<String>,
    /// Timestamp of soft deletion (ISO 8601), if deleted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
}

/// A historical edit of a message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageEdit {
    pub id: i64,
    pub message_id: String,
    pub old_content: String,
    pub edited_at: String,
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

/// Parameters for creating a new message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateMessageParams {
    pub channel_id: String,
    pub message_id: String,
    pub sender_pseudonym: String,
    pub content: String,
    pub reply_to_message_id: Option<String>,
}

pub(crate) fn map_row_to_channel(row: &Row) -> rusqlite::Result<Channel> {
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

pub(crate) fn map_row_to_member(row: &Row) -> rusqlite::Result<ChannelMember> {
    Ok(ChannelMember {
        id: row.get(0)?,
        server_id: row.get(1)?,
        channel_id: row.get(2)?,
        pseudonym_id: row.get(3)?,
        role: row.get(4)?,
        joined_at: row.get(5)?,
    })
}

pub(crate) fn map_row_to_message(row: &Row) -> rusqlite::Result<Message> {
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
        edited_at: row.get(9)?,
        deleted_at: row.get(10)?,
    })
}
