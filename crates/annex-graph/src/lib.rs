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

use annex_types::{EdgeKind, NodeType, RoleCode};
use rusqlite::{params, Connection};
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

/// An edge in the presence graph, representing a relationship.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    /// The database ID.
    pub id: i64,
    /// The server ID (scope).
    pub server_id: i64,
    /// The source node pseudonym.
    pub from_node: String,
    /// The target node pseudonym.
    pub to_node: String,
    /// The kind of relationship.
    pub kind: EdgeKind,
    /// The weight of the edge (default 1.0).
    pub weight: f64,
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

fn edge_kind_to_str(kind: EdgeKind) -> &'static str {
    match kind {
        EdgeKind::MemberOf => "MEMBER_OF",
        EdgeKind::Connected => "CONNECTED",
        EdgeKind::AgentServing => "AGENT_SERVING",
        EdgeKind::FederatedWith => "FEDERATED_WITH",
        EdgeKind::Moderates => "MODERATES",
    }
}

fn str_to_edge_kind(s: &str) -> EdgeKind {
    match s {
        "MEMBER_OF" => EdgeKind::MemberOf,
        "CONNECTED" => EdgeKind::Connected,
        "AGENT_SERVING" => EdgeKind::AgentServing,
        "FEDERATED_WITH" => EdgeKind::FederatedWith,
        "MODERATES" => EdgeKind::Moderates,
        _ => EdgeKind::Connected, // Fallback
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

/// Creates a new edge in the presence graph.
pub fn create_edge(
    conn: &Connection,
    server_id: i64,
    from_node: &str,
    to_node: &str,
    kind: EdgeKind,
    weight: f64,
) -> Result<GraphEdge, GraphError> {
    let kind_str = edge_kind_to_str(kind);

    let edge = conn.query_row(
        "INSERT INTO graph_edges (server_id, from_node, to_node, kind, weight)
         VALUES (?1, ?2, ?3, ?4, ?5)
         RETURNING id, server_id, from_node, to_node, kind, weight, created_at",
        params![server_id, from_node, to_node, kind_str, weight],
        |row| {
            let kind_str: String = row.get(4)?;
            let kind = str_to_edge_kind(&kind_str);
            Ok(GraphEdge {
                id: row.get(0)?,
                server_id: row.get(1)?,
                from_node: row.get(2)?,
                to_node: row.get(3)?,
                kind,
                weight: row.get(5)?,
                created_at: row.get(6)?,
            })
        },
    )?;

    Ok(edge)
}

/// Deletes an edge from the presence graph.
pub fn delete_edge(
    conn: &Connection,
    server_id: i64,
    from_node: &str,
    to_node: &str,
    kind: EdgeKind,
) -> Result<usize, GraphError> {
    let kind_str = edge_kind_to_str(kind);
    let count = conn.execute(
        "DELETE FROM graph_edges WHERE server_id = ?1 AND from_node = ?2 AND to_node = ?3 AND kind = ?4",
        params![server_id, from_node, to_node, kind_str],
    )?;
    Ok(count)
}

/// Retrieves all edges originating from a given node.
pub fn get_edges(
    conn: &Connection,
    server_id: i64,
    from_node: &str,
) -> Result<Vec<GraphEdge>, GraphError> {
    let mut stmt = conn.prepare(
        "SELECT id, server_id, from_node, to_node, kind, weight, created_at
         FROM graph_edges
         WHERE server_id = ?1 AND from_node = ?2",
    )?;

    let edge_iter = stmt.query_map(params![server_id, from_node], |row| {
        let kind_str: String = row.get(4)?;
        let kind = str_to_edge_kind(&kind_str);
        Ok(GraphEdge {
            id: row.get(0)?,
            server_id: row.get(1)?,
            from_node: row.get(2)?,
            to_node: row.get(3)?,
            kind,
            weight: row.get(5)?,
            created_at: row.get(6)?,
        })
    })?;

    let mut edges = Vec::new();
    for edge in edge_iter {
        edges.push(edge?);
    }

    Ok(edges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use annex_db::run_migrations;
    use annex_types::{EdgeKind, NodeType};
    use rusqlite::Connection;

    #[test]
    fn test_edge_lifecycle() {
        let conn = Connection::open_in_memory().expect("db open failed");
        run_migrations(&conn).expect("migrations failed");

        let server_id = 1;
        let node_a = "user_a";
        let node_b = "user_b";

        // Create nodes
        ensure_graph_node(&conn, server_id, node_a, NodeType::Human).expect("node a failed");
        ensure_graph_node(&conn, server_id, node_b, NodeType::Human).expect("node b failed");

        // Create edge
        let edge = create_edge(&conn, server_id, node_a, node_b, EdgeKind::Connected, 1.5)
            .expect("create_edge failed");

        assert_eq!(edge.from_node, node_a);
        assert_eq!(edge.to_node, node_b);
        assert_eq!(edge.kind, EdgeKind::Connected);
        assert_eq!(edge.weight, 1.5);

        // Get edges
        let edges = get_edges(&conn, server_id, node_a).expect("get_edges failed");
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].id, edge.id);

        // Delete edge
        let deleted = delete_edge(&conn, server_id, node_a, node_b, EdgeKind::Connected)
            .expect("delete_edge failed");
        assert_eq!(deleted, 1);

        // Verify empty
        let edges_after =
            get_edges(&conn, server_id, node_a).expect("get_edges after delete failed");
        assert!(edges_after.is_empty());
    }
}
