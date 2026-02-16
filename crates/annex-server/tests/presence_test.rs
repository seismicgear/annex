use annex_graph::{ensure_graph_node, prune_inactive_nodes, update_node_activity};
use annex_types::NodeType;
use rusqlite::Connection;
use std::thread;
use std::time::Duration;

#[test]
fn test_presence_logic() {
    let conn = Connection::open_in_memory().expect("db open failed");
    annex_db::run_migrations(&conn).expect("migrations failed");

    let server_id = 1;
    let pseudonym = "user_abc";

    // 1. Ensure node (active by default)
    ensure_graph_node(&conn, server_id, pseudonym, NodeType::Human).expect("ensure failed");

    // 2. Try update (already active) -> returns false
    let reactivated = update_node_activity(&conn, server_id, pseudonym).expect("update failed");
    assert!(!reactivated);

    // 3. Pruning check
    // Need threshold small enough to trigger pruning
    // Since last_seen_at is NOW, we need to wait > threshold.
    // Set threshold = 1 second. Wait 3s to ensure full second precision diff.
    thread::sleep(Duration::from_millis(3000));

    let pruned = prune_inactive_nodes(&conn, server_id, 1).expect("prune failed");
    assert_eq!(pruned.len(), 1);
    assert_eq!(pruned[0], pseudonym);

    // Verify inactive
    let active: bool = conn
        .query_row(
            "SELECT active FROM graph_nodes WHERE pseudonym_id = ?",
            [pseudonym],
            |row| row.get(0),
        )
        .unwrap();
    assert!(!active);

    // 4. Reactivate (inactive -> active) -> returns true
    let reactivated = update_node_activity(&conn, server_id, pseudonym).expect("update failed");
    assert!(reactivated);

    // Verify active
    let active: bool = conn
        .query_row(
            "SELECT active FROM graph_nodes WHERE pseudonym_id = ?",
            [pseudonym],
            |row| row.get(0),
        )
        .unwrap();
    assert!(active);
}
