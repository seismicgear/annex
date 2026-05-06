//! Channel CRUD: create, read, list (per-server and federated-only),
//! update (single-statement, partial), and delete (with explicit child-row
//! cleanup since SQLite cannot retrofit `ON DELETE CASCADE` via
//! `ALTER TABLE`).

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::ChannelError;
use crate::types::{map_row_to_channel, Channel, CreateChannelParams, UpdateChannelParams};

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

/// Lists channels for a given server (capped at 1000).
pub fn list_channels(conn: &Connection, server_id: i64) -> Result<Vec<Channel>, ChannelError> {
    let mut stmt = conn.prepare(
        "SELECT
            id, server_id, channel_id, name, channel_type, topic,
            vrp_topic_binding, required_capabilities_json, agent_min_alignment,
            retention_days, federation_scope, created_at
        FROM channels WHERE server_id = ?1 ORDER BY name ASC
        LIMIT 1000",
    )?;

    let rows = stmt.query_map([server_id], map_row_to_channel)?;
    let mut channels = Vec::new();
    for row in rows {
        channels.push(row?);
    }
    Ok(channels)
}

/// Lists all federated channels for a given server.
pub fn list_federated_channels(
    conn: &Connection,
    server_id: i64,
) -> Result<Vec<Channel>, ChannelError> {
    let federated_json = serde_json::to_string(&annex_types::FederationScope::Federated)?;

    let mut stmt = conn.prepare(
        "SELECT
            id, server_id, channel_id, name, channel_type, topic,
            vrp_topic_binding, required_capabilities_json, agent_min_alignment,
            retention_days, federation_scope, created_at
        FROM channels
        WHERE server_id = ?1 AND federation_scope = ?2
        ORDER BY name ASC",
    )?;

    let rows = stmt.query_map(params![server_id, federated_json], map_row_to_channel)?;
    let mut channels = Vec::new();
    for row in rows {
        channels.push(row?);
    }
    Ok(channels)
}

/// Updates an existing channel using a single atomic UPDATE statement.
///
/// Only fields that are `Some` in `updates` are modified; `None` fields are
/// left untouched. This avoids the read-modify-write race that would occur
/// if we fetched the channel, mutated in memory, and wrote back.
pub fn update_channel(
    conn: &Connection,
    channel_id: &str,
    updates: &UpdateChannelParams,
) -> Result<(), ChannelError> {
    let mut set_parts: Vec<String> = Vec::new();
    let mut values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
    let mut idx = 1usize;

    if let Some(name) = &updates.name {
        set_parts.push(format!("name = ?{idx}"));
        values.push(Box::new(name.clone()));
        idx += 1;
    }
    if let Some(topic) = &updates.topic {
        set_parts.push(format!("topic = ?{idx}"));
        values.push(Box::new(topic.clone()));
        idx += 1;
    }
    if let Some(binding) = &updates.vrp_topic_binding {
        set_parts.push(format!("vrp_topic_binding = ?{idx}"));
        values.push(Box::new(binding.clone()));
        idx += 1;
    }
    if let Some(caps) = &updates.required_capabilities_json {
        set_parts.push(format!("required_capabilities_json = ?{idx}"));
        values.push(Box::new(caps.clone()));
        idx += 1;
    }
    if let Some(align) = &updates.agent_min_alignment {
        let json = serde_json::to_string(align)?;
        set_parts.push(format!("agent_min_alignment = ?{idx}"));
        values.push(Box::new(json));
        idx += 1;
    }
    if let Some(days) = &updates.retention_days {
        set_parts.push(format!("retention_days = ?{idx}"));
        values.push(Box::new(*days));
        idx += 1;
    }
    if let Some(scope) = &updates.federation_scope {
        let json = serde_json::to_string(scope)?;
        set_parts.push(format!("federation_scope = ?{idx}"));
        values.push(Box::new(json));
        idx += 1;
    }

    if set_parts.is_empty() {
        // No fields to update; verify the channel exists for backward compat.
        let _ = get_channel(conn, channel_id)?;
        return Ok(());
    }

    let sql = format!(
        "UPDATE channels SET {} WHERE channel_id = ?{}",
        set_parts.join(", "),
        idx
    );
    values.push(Box::new(channel_id.to_string()));

    let params: Vec<&dyn rusqlite::types::ToSql> = values.iter().map(|v| v.as_ref()).collect();
    let count = conn.execute(&sql, params.as_slice())?;
    if count == 0 {
        return Err(ChannelError::NotFound(channel_id.to_string()));
    }
    Ok(())
}

/// Deletes a channel and all associated messages and members.
///
/// SQLite FK constraints on `messages.channel_id` and `channel_members.channel_id`
/// lack ON DELETE CASCADE (cannot be added via ALTER TABLE). We explicitly
/// delete child rows first within the same connection to ensure referential
/// integrity. The caller is expected to manage transaction boundaries if
/// atomicity with other operations is required.
pub fn delete_channel(conn: &Connection, channel_id: &str) -> Result<(), ChannelError> {
    // Wrap in a transaction so partial failures don't lose messages
    // while leaving the channel intact.
    let tx = conn.unchecked_transaction()?;

    // Delete child rows first to satisfy FK constraints.
    tx.execute("DELETE FROM messages WHERE channel_id = ?1", [channel_id])?;
    tx.execute(
        "DELETE FROM channel_members WHERE channel_id = ?1",
        [channel_id],
    )?;

    let count = tx.execute("DELETE FROM channels WHERE channel_id = ?1", [channel_id])?;
    if count == 0 {
        // Rollback is automatic on drop
        return Err(ChannelError::NotFound(channel_id.to_string()));
    }

    tx.commit()?;
    Ok(())
}
