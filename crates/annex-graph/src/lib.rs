//! Presence graph for the Annex platform.
//!
//! Implements the live presence graph: graph nodes (participants), edges
//! (relationships), BFS degree-of-separation queries, visibility rules,
//! SSE presence streaming, and activity-based pruning.
//!
//! The presence graph is how Annex represents who is connected, how they
//! relate to each other, and what they can see. Visibility is degree-based:
//! a participant at degree 1 sees more than a participant at degree 3.
//!
//! # Phase 5 implementation
//!
//! The full implementation of this crate is Phase 5 of the roadmap.

use annex_types::{NodeType, RoleCode};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors specific to the graph module.
#[derive(Debug, Error)]
pub enum GraphError {
    #[error("database error: {0}")]
    DatabaseError(#[from] rusqlite::Error),
    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
    #[error("node already exists: {0}")]
    NodeAlreadyExists(String),
}

/// A node in the presence graph, representing a participant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    /// The database ID.
    pub id: i64,
    /// The server ID (scope).
    pub server_id: i64,
    /// The pseudonym ID.
    pub pseudonym_id: String,
    /// The type of node (Human, Agent, etc.).
    pub node_type: NodeType,
    /// Whether the node is active.
    pub active: bool,
    /// Last seen timestamp (ISO 8601).
    pub last_seen_at: Option<String>,
    /// Additional metadata (JSON).
    pub metadata_json: Option<String>,
    /// Creation timestamp (ISO 8601).
    pub created_at: String,
}

/// Converts a `RoleCode` to a `NodeType`.
pub fn role_code_to_node_type(role: RoleCode) -> NodeType {
    match role {
        RoleCode::Human => NodeType::Human,
        RoleCode::AiAgent => NodeType::AiAgent,
        RoleCode::Collective => NodeType::Collective,
        RoleCode::Bridge => NodeType::Bridge,
        RoleCode::Service => NodeType::Service,
    }
}

/// Ensures a graph node exists for the given participant.
///
/// If the node already exists, it is updated to be active and its `last_seen_at`
/// timestamp is refreshed. If it does not exist, it is created.
///
/// This operation is idempotent.
pub fn ensure_graph_node(
    conn: &Connection,
    server_id: i64,
    pseudonym_id: &str,
    node_type: NodeType,
) -> Result<GraphNode, GraphError> {
    // Serialize node type
    let node_type_str = match node_type {
        NodeType::Human => "HUMAN",
        NodeType::AiAgent => "AI_AGENT",
        NodeType::Collective => "COLLECTIVE",
        NodeType::Bridge => "BRIDGE",
        NodeType::Service => "SERVICE",
    };

    // Upsert logic using ON CONFLICT DO UPDATE
    let node = conn.query_row(
        "INSERT INTO graph_nodes (server_id, pseudonym_id, node_type, active, last_seen_at)
         VALUES (?1, ?2, ?3, 1, datetime('now'))
         ON CONFLICT(server_id, pseudonym_id) DO UPDATE SET
            active = 1,
            last_seen_at = datetime('now')
         RETURNING id, server_id, pseudonym_id, node_type, active, last_seen_at, metadata_json, created_at",
        (server_id, pseudonym_id, node_type_str),
        |row| {
            let node_type_str: String = row.get(3)?;
            let node_type = match node_type_str.as_str() {
                "HUMAN" => NodeType::Human,
                "AI_AGENT" => NodeType::AiAgent,
                "COLLECTIVE" => NodeType::Collective,
                "BRIDGE" => NodeType::Bridge,
                "SERVICE" => NodeType::Service,
                _ => NodeType::Human, // Default fallback
            };

            Ok(GraphNode {
                id: row.get(0)?,
                server_id: row.get(1)?,
                pseudonym_id: row.get(2)?,
                node_type,
                active: row.get(4)?,
                last_seen_at: row.get(5)?,
                metadata_json: row.get(6)?,
                created_at: row.get(7)?,
            })
        },
    ).map_err(GraphError::DatabaseError)?;

    Ok(node)
}
