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

use annex_types::{EdgeKind, NodeType, RoleCode, VisibilityLevel};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
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
    #[error("node not found: {0}")]
    NodeNotFound(String),
}

/// A filtered view of a graph node, respecting visibility rules.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphProfile {
    pub pseudonym_id: String,
    pub node_type: NodeType,
    pub active: bool,
    pub created_at: String,
    /// Only visible if visibility >= Degree1 (or Self).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_seen_at: Option<String>,
    /// Only visible if visibility == Self or specific permission (not implemented yet, so Self only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_json: Option<String>,
    pub visibility: VisibilityLevel,
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

/// Result of a BFS path finding operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BfsPath {
    /// Whether a path was found.
    pub found: bool,
    /// The sequence of pseudonyms from start to end.
    pub path: Vec<String>,
    /// The length of the path (number of edges).
    pub length: usize,
}

/// Updates the last seen timestamp of a node and ensures it is active.
///
/// Returns `true` if the node was previously inactive, allowing the caller to emit a reactivation event.
/// Returns `false` if the node was already active or does not exist.
pub fn update_node_activity(
    conn: &Connection,
    server_id: i64,
    pseudonym_id: &str,
) -> Result<bool, GraphError> {
    // 1. Try updating an already-active node (most common case).
    let count = conn
        .execute(
            "UPDATE graph_nodes
             SET last_seen_at = datetime('now')
             WHERE server_id = ?1 AND pseudonym_id = ?2 AND active = 1",
            params![server_id, pseudonym_id],
        )
        .map_err(GraphError::DatabaseError)?;

    if count > 0 {
        return Ok(false); // Was already active
    }

    // 2. If not found, try activating an inactive node.
    let count = conn
        .execute(
            "UPDATE graph_nodes
             SET active = 1, last_seen_at = datetime('now')
             WHERE server_id = ?1 AND pseudonym_id = ?2 AND active = 0",
            params![server_id, pseudonym_id],
        )
        .map_err(GraphError::DatabaseError)?;

    if count > 0 {
        return Ok(true); // Was inactive, now active
    }

    // Node does not exist
    Ok(false)
}

/// Prunes inactive nodes based on a threshold.
///
/// Sets `active = 0` for nodes where `last_seen_at` is older than the threshold
/// and the node is currently active.
///
/// Returns a list of pseudonyms that were pruned.
pub fn prune_inactive_nodes(
    conn: &Connection,
    server_id: i64,
    threshold_seconds: u64,
) -> Result<Vec<String>, GraphError> {
    let mut stmt = conn.prepare(
        "UPDATE graph_nodes
         SET active = 0
         WHERE server_id = ?1
           AND active = 1
           AND last_seen_at < datetime('now', '-' || ?2 || ' seconds')
         RETURNING pseudonym_id",
    )?;

    let rows = stmt.query_map(params![server_id, threshold_seconds], |row| {
        row.get::<_, String>(0)
    })?;

    let mut pruned = Vec::new();
    for r in rows {
        pruned.push(r.map_err(GraphError::DatabaseError)?);
    }

    Ok(pruned)
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

/// Retrieves a graph node by pseudonym.
pub fn get_graph_node(
    conn: &Connection,
    server_id: i64,
    pseudonym_id: &str,
) -> Result<Option<GraphNode>, GraphError> {
    conn.query_row(
        "SELECT id, server_id, pseudonym_id, node_type, active, last_seen_at, metadata_json, created_at
         FROM graph_nodes
         WHERE server_id = ?1 AND pseudonym_id = ?2",
        params![server_id, pseudonym_id],
        |row| {
            let node_type_str: String = row.get(3)?;
            let node_type = match node_type_str.as_str() {
                "HUMAN" => NodeType::Human,
                "AI_AGENT" => NodeType::AiAgent,
                "COLLECTIVE" => NodeType::Collective,
                "BRIDGE" => NodeType::Bridge,
                "SERVICE" => NodeType::Service,
                _ => NodeType::Human,
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
    )
    .optional()
    .map_err(GraphError::DatabaseError)
}

/// Calculates the visibility level of a target node from the perspective of a viewer.
pub fn get_node_visibility(
    conn: &Connection,
    server_id: i64,
    viewer: &str,
    target: &str,
) -> Result<VisibilityLevel, GraphError> {
    if viewer == target {
        return Ok(VisibilityLevel::Self_);
    }

    // Max depth 3 for visibility calculation
    let path = find_path_bfs(conn, server_id, viewer, target, 3)?;

    if !path.found {
        return Ok(VisibilityLevel::None);
    }

    match path.length {
        0 => Ok(VisibilityLevel::Self_), // Should be covered by viewer == target, but just in case
        1 => Ok(VisibilityLevel::Degree1),
        2 => Ok(VisibilityLevel::Degree2),
        3 => Ok(VisibilityLevel::Degree3),
        _ => Ok(VisibilityLevel::None), // Should not happen given max_depth=3
    }
}

/// Retrieves the profile of a target node visible to the viewer.
pub fn get_visible_profile(
    conn: &Connection,
    server_id: i64,
    viewer: &str,
    target: &str,
) -> Result<GraphProfile, GraphError> {
    let node = get_graph_node(conn, server_id, target)?
        .ok_or_else(|| GraphError::NodeNotFound(target.to_string()))?;

    let visibility = get_node_visibility(conn, server_id, viewer, target)?;

    let (last_seen_at, metadata_json) = match visibility {
        VisibilityLevel::Self_ => (node.last_seen_at, node.metadata_json),
        VisibilityLevel::Degree1 => (node.last_seen_at, None), // Hide metadata for degree 1
        VisibilityLevel::Degree2 => (None, None),              // Hide last_seen and metadata
        VisibilityLevel::Degree3 => (None, None),              // Hide last_seen and metadata
        VisibilityLevel::AggregateOnly => (None, None),
        VisibilityLevel::None => (None, None),
    };

    Ok(GraphProfile {
        pseudonym_id: node.pseudonym_id,
        node_type: node.node_type,
        active: node.active,
        created_at: node.created_at,
        last_seen_at,
        metadata_json,
        visibility,
    })
}

/// Finds the shortest path between two nodes using BFS.
///
/// Treats the graph as undirected (checks both `from->to` and `to->from` edges).
pub fn find_path_bfs(
    conn: &Connection,
    server_id: i64,
    from_node: &str,
    to_node: &str,
    max_depth: u32,
) -> Result<BfsPath, GraphError> {
    if from_node == to_node {
        return Ok(BfsPath {
            found: true,
            path: vec![from_node.to_string()],
            length: 0,
        });
    }

    let mut queue = VecDeque::new();
    queue.push_back((from_node.to_string(), vec![from_node.to_string()]));

    let mut visited = HashSet::new();
    visited.insert(from_node.to_string());

    // Prepare statements for neighbor lookup (undirected)
    let mut stmt_out =
        conn.prepare("SELECT to_node FROM graph_edges WHERE server_id = ?1 AND from_node = ?2")?;
    let mut stmt_in =
        conn.prepare("SELECT from_node FROM graph_edges WHERE server_id = ?1 AND to_node = ?2")?;

    while let Some((current_node, current_path)) = queue.pop_front() {
        if current_path.len() > max_depth as usize {
            continue;
        }

        let mut neighbors = Vec::new();

        // Outgoing edges
        let rows_out = stmt_out.query_map(params![server_id, current_node], |row| {
            row.get::<_, String>(0)
        })?;
        for r in rows_out {
            neighbors.push(r?);
        }

        // Incoming edges
        let rows_in = stmt_in.query_map(params![server_id, current_node], |row| {
            row.get::<_, String>(0)
        })?;
        for r in rows_in {
            neighbors.push(r?);
        }

        for neighbor in neighbors {
            if neighbor == to_node {
                let mut new_path = current_path.clone();
                new_path.push(neighbor);
                return Ok(BfsPath {
                    found: true,
                    length: new_path.len() - 1,
                    path: new_path,
                });
            }

            if !visited.contains(&neighbor) {
                visited.insert(neighbor.clone());
                let mut new_path = current_path.clone();
                new_path.push(neighbor.clone());
                queue.push_back((neighbor, new_path));
            }
        }
    }

    Ok(BfsPath {
        found: false,
        path: Vec::new(),
        length: 0,
    })
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

    #[test]
    fn test_bfs() {
        let conn = Connection::open_in_memory().expect("db open failed");
        run_migrations(&conn).expect("migrations failed");
        let server_id = 1;

        // Graph: A -> B -> C, and A -> D
        let nodes = vec!["A", "B", "C", "D"];
        for n in &nodes {
            ensure_graph_node(&conn, server_id, n, NodeType::Human).unwrap();
        }

        create_edge(&conn, server_id, "A", "B", EdgeKind::Connected, 1.0).unwrap();
        create_edge(&conn, server_id, "B", "C", EdgeKind::Connected, 1.0).unwrap();
        create_edge(&conn, server_id, "A", "D", EdgeKind::Connected, 1.0).unwrap();

        // 1. Direct path A -> B
        let path = find_path_bfs(&conn, server_id, "A", "B", 5).unwrap();
        assert!(path.found);
        assert_eq!(path.path, vec!["A", "B"]);
        assert_eq!(path.length, 1);

        // 2. Path A -> C (length 2)
        let path = find_path_bfs(&conn, server_id, "A", "C", 5).unwrap();
        assert!(path.found);
        assert_eq!(path.path, vec!["A", "B", "C"]);
        assert_eq!(path.length, 2);

        // 3. Undirected check: C -> A
        let path = find_path_bfs(&conn, server_id, "C", "A", 5).unwrap();
        assert!(path.found);
        assert_eq!(path.path, vec!["C", "B", "A"]); // Should traverse back
        assert_eq!(path.length, 2);

        // 4. Max depth exceeded
        let path = find_path_bfs(&conn, server_id, "A", "C", 1).unwrap();
        assert!(!path.found);

        // 5. No path
        ensure_graph_node(&conn, server_id, "Z", NodeType::Human).unwrap();
        let path = find_path_bfs(&conn, server_id, "A", "Z", 5).unwrap();
        assert!(!path.found);

        // 6. Self path
        let path = find_path_bfs(&conn, server_id, "A", "A", 5).unwrap();
        assert!(path.found);
        assert_eq!(path.length, 0);
        assert_eq!(path.path, vec!["A"]);
    }
}
