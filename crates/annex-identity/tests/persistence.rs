use annex_db::run_migrations;
use annex_identity::{merkle::MerkleTree, Fr};
use rusqlite::Connection;

#[test]
fn test_merkle_persistence_round_trip() {
    let conn = Connection::open_in_memory().expect("failed to open in-memory db");
    run_migrations(&conn).expect("migrations failed");

    let depth = 3;
    let mut tree = MerkleTree::new(depth).expect("failed to create tree");

    // Insert some leaves
    let leaf1 = Fr::from(100);
    let index1 = tree.insert(leaf1).expect("failed to insert leaf1");
    tree.persist_leaf(&conn, index1, leaf1).expect("failed to persist leaf1");

    let leaf2 = Fr::from(200);
    let index2 = tree.insert(leaf2).expect("failed to insert leaf2");
    tree.persist_leaf(&conn, index2, leaf2).expect("failed to persist leaf2");

    // Persist root
    tree.persist_root(&conn).expect("failed to persist root");

    // "Restart" - create new tree from DB
    let restored_tree = MerkleTree::restore(&conn, depth).expect("failed to restore tree");

    // Verify consistency
    assert_eq!(tree.root(), restored_tree.root());

    // Verify proofs work on restored tree
    let (proof1, indices1) = restored_tree.get_proof(index1).expect("failed to get proof1");
    assert_eq!(proof1.len(), depth);
    assert_eq!(indices1.len(), depth);

    // Verify insert on restored tree works
    let mut restored_tree = restored_tree; // mut
    let leaf3 = Fr::from(300);
    let index3 = restored_tree.insert(leaf3).expect("failed to insert leaf3");
    restored_tree.persist_leaf(&conn, index3, leaf3).expect("failed to persist leaf3");
    restored_tree.persist_root(&conn).expect("failed to persist new root");

    assert_ne!(tree.root(), restored_tree.root());
}

#[test]
fn test_restore_recovers_on_root_mismatch() {
    let conn = Connection::open_in_memory().expect("failed to open in-memory db");
    run_migrations(&conn).expect("migrations failed");

    let depth = 3;
    let mut tree = MerkleTree::new(depth).expect("failed to create tree");
    let leaf = Fr::from(100);
    let index = tree.insert(leaf).expect("failed to insert");
    tree.persist_leaf(&conn, index, leaf).expect("failed to persist leaf");
    tree.persist_root(&conn).expect("failed to persist root");

    // Tamper with the root in DB
    conn.execute(
        "UPDATE vrp_roots SET root_hex = 'deadbeef' WHERE active = 1",
        [],
    )
    .expect("tamper failed");

    // Attempt restore
    let res = MerkleTree::restore(&conn, depth);
    assert!(res.is_ok(), "Restore should recover from root mismatch");
}
