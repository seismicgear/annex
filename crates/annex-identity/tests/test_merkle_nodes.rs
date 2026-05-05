//! Persisted-Merkle-node + root-epoch tests for migration 034.
//!
//! These cover the 8 required behaviours called out in the task:
//!   1. Fresh DB initialises Merkle meta.
//!   2. Insert persists path nodes.
//!   3. Restart loads from nodes without replaying all leaves.
//!   4. Rebuild-from-leaves matches persisted root.
//!   5. Recent root accepted within grace window.
//!   6. Ancient root rejected.
//!   7. Concurrent registration does not corrupt next_index.
//!   8. Tree depth mismatch fails startup clearly.

use annex_db::run_migrations;
use annex_identity::merkle::{is_root_acceptable, load_meta, MerkleTree, ROOT_EPOCH_GRACE_SECONDS};
use annex_identity::IdentityError;
use ark_bn254::Fr;
use rusqlite::Connection;

fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open db");
    run_migrations(&conn).expect("migrations");
    conn
}

#[test]
fn fresh_db_restore_initialises_merkle_meta() {
    // Migration 034 creates the tables but does not insert any rows.
    // `MerkleTree::restore` on a fresh DB must seed the singleton meta
    // row so that subsequent restart paths take the fast lane.
    let conn = fresh_db();

    // Sanity: the table is empty before restore.
    let pre = load_meta(&conn).expect("load_meta");
    assert!(pre.is_none(), "fresh DB should have no meta row yet");

    let tree = MerkleTree::restore(&conn, 5).expect("restore");
    assert_eq!(tree.depth, 5);
    assert_eq!(tree.next_index, 0);

    let meta = load_meta(&conn).expect("meta").expect("meta seeded");
    assert_eq!(meta.tree_depth, 5);
    assert_eq!(meta.next_index, 0);
    assert_eq!(meta.current_root_hex.len(), 64);
}

#[test]
fn insert_persists_only_the_touched_path_nodes() {
    let mut conn = fresh_db();

    let mut tree = MerkleTree::restore(&conn, 5).expect("restore");
    tree.insert_and_persist(&mut conn, Fr::from(101u64))
        .expect("insert leaf 0");

    // Path length = depth + 1 (one entry per level, from leaf to root).
    let row_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM vrp_merkle_nodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        row_count,
        (tree.depth + 1) as i64,
        "after one insert into a depth-{} tree, exactly {} nodes must be persisted",
        tree.depth,
        tree.depth + 1,
    );

    // Inserting a second leaf at index 1: the leaf itself is a new node
    // at (0, 1), but every internal-level node along its path lives at
    // node_index 0 (since 1 // 2 = 0, 0 // 2 = 0, …) — exactly the same
    // (level, node_index) coordinates the first insert wrote. INSERT OR
    // REPLACE updates them in place; the only NEW row is the leaf.
    tree.insert_and_persist(&mut conn, Fr::from(202u64))
        .expect("insert leaf 1");
    let row_count_2: i64 = conn
        .query_row("SELECT COUNT(*) FROM vrp_merkle_nodes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        row_count_2,
        (tree.depth + 2) as i64,
        "second leaf adds exactly one new row (the leaf at (0,1)); the path nodes \
         all share (level, node_index) with the first insert and are updated in place"
    );
}

#[test]
fn restart_loads_from_nodes_without_replaying_leaves() {
    // Insert several leaves, drop the in-memory tree, then verify that
    // a fresh `restore` returns the same root after loading from
    // `vrp_merkle_meta` + `vrp_merkle_nodes` — without re-hashing the
    // leaf set.
    let mut conn = fresh_db();
    let depth = 5;
    let mut tree = MerkleTree::restore(&conn, depth).expect("restore-1");
    for v in 1u64..=4 {
        tree.insert_and_persist(&mut conn, Fr::from(v))
            .expect("insert");
    }
    let expected_root_hex = annex_identity::zk::fr_to_canonical_hex(tree.root());
    let expected_next = tree.next_index;
    drop(tree);

    // Knock the leaf table flat to PROVE the restore path doesn't replay
    // it. If the fast path needed leaves, this would corrupt the tree.
    conn.execute("DELETE FROM vrp_leaves", []).unwrap();

    let restored = MerkleTree::restore(&conn, depth).expect("restore-2");
    assert_eq!(restored.next_index, expected_next);
    assert_eq!(
        annex_identity::zk::fr_to_canonical_hex(restored.root()),
        expected_root_hex,
        "fast-path restore must reproduce the same root"
    );
}

#[test]
fn rebuild_from_leaves_matches_persisted_root() {
    let mut conn = fresh_db();
    let depth = 5;
    let mut tree = MerkleTree::restore(&conn, depth).expect("restore");
    for v in [11u64, 22, 33, 44, 55] {
        tree.insert_and_persist(&mut conn, Fr::from(v))
            .expect("insert");
    }
    let persisted = annex_identity::zk::fr_to_canonical_hex(tree.root());
    let recomputed = MerkleTree::audit_against_leaves(&conn, depth).expect("audit_against_leaves");
    assert_eq!(
        persisted, recomputed,
        "rebuild from vrp_leaves must reproduce the persisted root exactly"
    );
}

#[test]
fn recent_root_accepted_within_grace_window() {
    let mut conn = fresh_db();
    let depth = 5;
    let mut tree = MerkleTree::restore(&conn, depth).expect("restore");

    // Insert one leaf -> root R0.
    tree.insert_and_persist(&mut conn, Fr::from(1u64)).unwrap();
    let r0 = annex_identity::zk::fr_to_canonical_hex(tree.root());
    // R0 is currently active.
    assert!(is_root_acceptable(&conn, &r0).unwrap());

    // Rotate by inserting a second leaf -> root R1. R0 now has
    // active_until = now and accepted_until = now + GRACE.
    tree.insert_and_persist(&mut conn, Fr::from(2u64)).unwrap();
    let r1 = annex_identity::zk::fr_to_canonical_hex(tree.root());
    assert!(
        is_root_acceptable(&conn, &r1).unwrap(),
        "current root accepted"
    );
    assert!(
        is_root_acceptable(&conn, &r0).unwrap(),
        "previous root must remain acceptable while accepted_until > now"
    );
    // Sanity: GRACE constant is non-trivial. Compile-time const_assert
    // would be cleaner, but a runtime check is enough — the value is a
    // public const, so the comparison is constant-folded at compile time.
    let _grace_window: i64 = ROOT_EPOCH_GRACE_SECONDS;
    assert!(_grace_window >= 60, "grace window must allow >= 1 minute");
}

#[test]
fn ancient_root_rejected() {
    let mut conn = fresh_db();
    let depth = 5;
    let mut tree = MerkleTree::restore(&conn, depth).expect("restore");
    tree.insert_and_persist(&mut conn, Fr::from(1u64)).unwrap();
    let r0 = annex_identity::zk::fr_to_canonical_hex(tree.root());
    tree.insert_and_persist(&mut conn, Fr::from(2u64)).unwrap();

    // Force the grace window for R0 closed by setting accepted_until to a
    // historical timestamp. This simulates "the rotation happened long
    // enough ago that the grace window has expired".
    conn.execute(
        "UPDATE vrp_root_epochs \
         SET active_until = '2000-01-01 00:00:00', \
             accepted_until = '2000-01-01 00:05:00' \
         WHERE root_hex = ?1",
        [&r0],
    )
    .unwrap();

    assert!(
        !is_root_acceptable(&conn, &r0).unwrap(),
        "ancient (post-grace-window) root must be rejected"
    );

    // Also: a root that has never been recorded at all is rejected.
    let unknown = "0".repeat(64);
    assert!(
        !is_root_acceptable(&conn, &unknown).unwrap(),
        "unknown root must be rejected"
    );
}

#[test]
fn concurrent_registration_does_not_corrupt_next_index() {
    // SQLite serialises writers per database. Spawn N threads that each
    // open their own connection to a shared file-backed DB and try to
    // race insertions. Every successful insert must observe a unique
    // next_index, and the final state must equal the count of successes.
    use rusqlite::Connection;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::thread;

    let dir = std::env::temp_dir().join(format!(
        "annex-merkle-conc-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let db_path: PathBuf = dir.join("test.db");
    {
        let setup = Connection::open(&db_path).unwrap();
        run_migrations(&setup).unwrap();
        // Bootstrap meta + zero-leaf state via restore.
        let _ = MerkleTree::restore(&setup, 5).expect("restore");
    }

    const N: u64 = 8;
    let path = Arc::new(db_path.clone());
    let handles: Vec<_> = (0..N)
        .map(|i| {
            let path = path.clone();
            thread::spawn(move || -> Result<usize, String> {
                let mut conn = Connection::open(&*path).map_err(|e| e.to_string())?;
                // SQLite's default lock contention can show up as
                // BUSY/LOCKED errors under multi-writer pressure. Set a
                // generous busy_timeout so the race serialises rather
                // than fails noisily.
                conn.busy_timeout(std::time::Duration::from_secs(5))
                    .map_err(|e| e.to_string())?;
                let mut tree = MerkleTree::restore(&conn, 5).map_err(|e| e.to_string())?;
                tree.insert_and_persist(&mut conn, Fr::from(1000u64 + i))
                    .map_err(|e| e.to_string())
            })
        })
        .collect();

    let successful: usize = handles
        .into_iter()
        .filter_map(|h| h.join().unwrap().ok())
        .count();
    assert!(successful >= 1);

    let conn = Connection::open(&db_path).unwrap();
    let leaf_count: i64 = conn
        .query_row("SELECT COUNT(*) FROM vrp_leaves", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        leaf_count as usize, successful,
        "every successful insert must have produced exactly one row in vrp_leaves"
    );

    // Indices must form the contiguous range [0, successful).
    let mut indices: Vec<i64> = conn
        .prepare("SELECT leaf_index FROM vrp_leaves ORDER BY leaf_index ASC")
        .unwrap()
        .query_map([], |r| r.get::<_, i64>(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    indices.dedup();
    assert_eq!(
        indices,
        (0..successful as i64).collect::<Vec<_>>(),
        "indices must be unique and contiguous; observed: {indices:?}"
    );

    let meta = load_meta(&conn).unwrap().unwrap();
    assert_eq!(
        meta.next_index, successful,
        "meta.next_index must equal the number of successfully-inserted leaves"
    );

    // Final root must agree between meta and a fresh rebuild from leaves.
    let recomputed = MerkleTree::audit_against_leaves(&conn, 5).unwrap();
    assert_eq!(meta.current_root_hex, recomputed);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tree_depth_mismatch_fails_startup_clearly() {
    let mut conn = fresh_db();
    let mut tree = MerkleTree::restore(&conn, 5).expect("restore at depth 5");
    tree.insert_and_persist(&mut conn, Fr::from(123u64))
        .expect("insert");
    drop(tree);

    // Now try to restore the SAME database at a different depth. This
    // would silently re-shard the tree if not caught — and would
    // invalidate every previously-issued proof.
    let result = MerkleTree::restore(&conn, 8);
    match result {
        Err(IdentityError::MerkleTreeDepthMismatch { stored, configured }) => {
            assert_eq!(stored, 5);
            assert_eq!(configured, 8);
        }
        Err(other) => panic!("expected MerkleTreeDepthMismatch, got: {other:?}"),
        Ok(_) => panic!("restore must refuse to load a depth-5 tree as depth-8"),
    }
}
