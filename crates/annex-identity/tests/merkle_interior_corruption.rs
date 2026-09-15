//! A corrupted interior node must be detected, not served.
//!
//! `MerkleTree::restore` takes a fast path: it loads every row from
//! `vrp_merkle_nodes` and compares the TOP node against
//! `vrp_merkle_meta.current_root_hex`. It does not recompute the interior. So a
//! node damaged at a lower level — by a partial write, a bad disk, or someone
//! with database access — restores cleanly as long as those two values are
//! left intact.
//!
//! Nothing then said the tree was wrong. Proofs drawn through the damaged node
//! simply failed verification, which is a symptom arriving at the user rather
//! than a diagnosis arriving at the operator.
//!
//! `get_proof` now recomputes the root from the leaf and the path it is about
//! to hand out. Twenty hashes per proof, and it covers every path anyone
//! actually relies on rather than only the state at boot.

use annex_db::run_migrations;
use annex_identity::merkle::MerkleTree;
use annex_identity::IdentityError;
use ark_bn254::Fr;
use rusqlite::Connection;

/// Shallow on purpose: the property is about interior nodes, and depth 5 makes
/// the damaged node's position obvious in the assertions below.
const DEPTH: usize = 5;

/// A persisted tree with `n` leaves, and the connection it lives in.
fn seeded_tree(n: u64) -> (Connection, MerkleTree) {
    let mut conn = Connection::open_in_memory().expect("open db");
    run_migrations(&conn).expect("migrations");
    let mut tree = MerkleTree::restore(&conn, DEPTH).expect("restore");
    for i in 0..n {
        tree.insert_and_persist(&mut conn, Fr::from(100 + i))
            .expect("insert");
    }
    (conn, tree)
}

#[test]
fn an_honest_tree_produces_proofs_that_reach_its_root() {
    let (_conn, tree) = seeded_tree(8);
    for i in 0..8usize {
        let proof = tree.get_proof(i);
        assert!(
            proof.is_ok(),
            "leaf {i} should produce a proof: {:?}",
            proof.err()
        );
    }
}

#[test]
fn a_corrupted_interior_node_is_caught_when_a_proof_is_drawn_through_it() {
    let (conn, _tree) = seeded_tree(8);

    // Damage ONE interior node, leaving the leaves, the top node and the
    // metadata root untouched. This is precisely the state the fast restore
    // path cannot see: it compares the top node with the stored root, and both
    // still agree.
    //
    // WHICH node, and which leaf is then queried, is the whole test. A proof
    // RECOMPUTES the nodes on its own path and READS only the siblings, so
    // corrupting node (1,0) does not affect a proof for leaf 0 or 1 — their
    // proofs rebuild that node from the leaves and are still correct. It is
    // leaves 2 and 3 whose level-1 sibling IS (1,0), so they are the ones that
    // draw the damaged value in.
    //
    // The first version of this test corrupted (1,0) and queried leaf 0, then
    // reported that the check had failed to fire. The check was right and the
    // test was pointed at the wrong leaf.
    {
        let changed = conn
            .execute(
                "UPDATE vrp_merkle_nodes
                    SET hash_hex = '0000000000000000000000000000000000000000000000000000000000000002'
                  WHERE level = 1 AND node_index = 0",
                [],
            )
            .unwrap();
        assert_eq!(changed, 1, "the fixture must actually damage a node");
    }

    // Restore succeeds — that is the point, and why this needed a separate
    // check at all.
    let restored = MerkleTree::restore(&conn, DEPTH);
    assert!(
        restored.is_ok(),
        "restore is expected to ACCEPT this database; if it starts rejecting it, \
         this test is no longer testing what it says: {:?}",
        restored.err()
    );
    let restored = restored.unwrap();

    // Leaf 2's level-1 sibling is the damaged node (1,0).
    match restored.get_proof(2) {
        Err(IdentityError::MerkleRootMismatch { stored, computed }) => {
            assert_ne!(
                stored, computed,
                "a mismatch error should carry two different roots"
            );
        }
        Err(other) => panic!("expected MerkleRootMismatch, got {other:?}"),
        Ok(_) => panic!(
            "a proof was drawn through a corrupted interior node and handed out as valid. \
             It would fail verification at the user, with nothing saying the server's \
             tree is damaged."
        ),
    }
}

/// Damage must stay local. A check that rejected every proof once anything was
/// wrong would be useless in a different way — and it would make the error
/// useless as a pointer to WHERE the damage is.
#[test]
fn proofs_on_untouched_branches_still_work() {
    let (conn, _tree) = seeded_tree(8);
    {
        // Level 1, index 0 covers leaves 0-1. Leaves 4-7 descend through
        // level 1 index 2 and 3, which are untouched.
        conn.execute(
            "UPDATE vrp_merkle_nodes
                SET hash_hex = '0000000000000000000000000000000000000000000000000000000000000002'
              WHERE level = 1 AND node_index = 0",
            [],
        )
        .unwrap();
    }
    let restored = MerkleTree::restore(&conn, DEPTH).unwrap();

    // Leaves 4-7 descend through (1,2) and (1,3) and take (2,0) as their
    // level-2 sibling — none of which was touched. Their proofs are unaffected
    // and must still be served.
    for leaf in [4usize, 5, 6, 7] {
        assert!(
            restored.get_proof(leaf).is_ok(),
            "leaf {leaf} is on an undamaged branch and must still produce a proof"
        );
    }

    // And leaves 0 and 1, whose proofs RECOMPUTE the damaged node rather than
    // reading it, are also unaffected. Worth pinning: it is the reason the
    // check cannot be described as "detects a corrupted tree" — it detects a
    // corrupted value that a proof actually depends on.
    for leaf in [0usize, 1] {
        assert!(
            restored.get_proof(leaf).is_ok(),
            "leaf {leaf} rebuilds the damaged node from its own path and should still verify"
        );
    }

    // Leaves 2 and 3 are the ones that read it.
    for leaf in [2usize, 3] {
        match restored.get_proof(leaf) {
            Err(IdentityError::MerkleRootMismatch { .. }) => {}
            other => panic!("leaf {leaf}: expected a root mismatch, got {other:?}"),
        }
    }
}

/// An index that was never inserted is an invalid index, not a corruption
/// report. Collapsing the two would send an operator hunting a disk fault
/// over a bad request.
#[test]
fn an_out_of_range_index_is_still_an_invalid_index() {
    let (_conn, tree) = seeded_tree(4);
    match tree.get_proof(99) {
        Err(IdentityError::InvalidIndex(99)) => {}
        other => panic!("expected InvalidIndex(99), got {other:?}"),
    }
}
