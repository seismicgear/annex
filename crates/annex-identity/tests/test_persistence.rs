use annex_db::run_migrations;
use annex_identity::MerkleTree;
use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};
use rusqlite::Connection;

#[test]
fn test_merkle_persistence_roundtrip() {
    // 1. Setup in-memory DB
    let mut conn = Connection::open_in_memory().expect("failed to open DB");
    run_migrations(&conn).expect("migrations failed");

    // 2. Create tree and insert leaves
    let mut tree = MerkleTree::new(3).expect("failed to create tree");
    let leaf1 = Fr::from(100);
    let leaf2 = Fr::from(200);

    tree.insert_and_persist(&mut conn, leaf1)
        .expect("failed to insert leaf1");
    tree.insert_and_persist(&mut conn, leaf2)
        .expect("failed to insert leaf2");

    let original_root = tree.root();
    let original_root_hex = hex::encode(original_root.into_bigint().to_bytes_be());

    // 3. Verify DB content
    let stored_root: String = conn
        .query_row(
            "SELECT root_hex FROM vrp_roots WHERE active = 1",
            [],
            |row| row.get(0),
        )
        .expect("should have active root");

    assert_eq!(stored_root, original_root_hex);

    let leaf_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM vrp_leaves", [], |row| row.get(0))
        .expect("should count leaves");
    assert_eq!(leaf_count, 2);

    // 4. Restore tree from DB
    let mut restored_tree = MerkleTree::restore(&conn, 3).expect("failed to restore tree");

    // 5. Verify restored tree matches
    assert_eq!(restored_tree.root(), original_root);
    assert_eq!(restored_tree.next_index, 2);

    // 6. Insert more leaves to verify restored tree is functional
    let leaf3 = Fr::from(300);
    restored_tree
        .insert_and_persist(&mut conn, leaf3)
        .expect("failed to insert leaf3");

    assert_eq!(restored_tree.next_index, 3);
    assert_ne!(restored_tree.root(), original_root);

    // 7. Verify proof works on restored tree
    let (path_elements, _) = restored_tree
        .get_proof(0)
        .expect("failed to get proof for leaf 0");
    assert_eq!(path_elements.len(), 3);
}

#[test]
fn test_restore_prioritizes_computed_root() {
    // 1. Setup in-memory DB
    let conn = Connection::open_in_memory().expect("failed to open DB");
    run_migrations(&conn).expect("migrations failed");

    // 2. Manually insert inconsistent state
    // Insert a leaf
    let leaf = Fr::from(100);
    let leaf_bytes = leaf.into_bigint().to_bytes_be();
    let leaf_hex = hex::encode(leaf_bytes);

    conn.execute(
        "INSERT INTO vrp_leaves (leaf_index, commitment_hex) VALUES (0, ?1)",
        [&leaf_hex],
    )
    .unwrap();

    // Insert a fake root that doesn't match
    let fake_root = "000000000000000000000000000000000000000000000000000000000000dead";
    conn.execute(
        "INSERT INTO vrp_roots (root_hex, active) VALUES (?1, 1)",
        [fake_root],
    )
    .unwrap();

    // 3. Restore tree
    let tree = MerkleTree::restore(&conn, 3).expect("should succeed despite mismatch");

    // 4. Verify tree root is computed correctly (not fake_root)
    let computed_root = tree.root();
    let computed_root_hex = hex::encode(computed_root.into_bigint().to_bytes_be());

    assert_ne!(computed_root_hex, fake_root);
}
