/// Performance test: Merkle tree operations.
///
/// Target: insert + proof < 50ms for trees approaching 1M leaves.
/// This test inserts a smaller count (1000 leaves) and extrapolates,
/// as full 1M-leaf tests take too long for CI.
use annex_identity::MerkleTree;
use ark_bn254::Fr;
use std::time::Instant;

#[test]
fn perf_merkle_insert_and_proof_1000_leaves() {
    let depth = 20; // Supports up to 2^20 = 1,048,576 leaves
    let mut tree = MerkleTree::new(depth).expect("failed to create tree");
    let n = 1000;

    // Insert 1000 leaves and time it
    let insert_start = Instant::now();
    for i in 0..n {
        let leaf = Fr::from((i + 1) as u64);
        tree.insert(leaf).expect("insert failed");
    }
    let insert_elapsed = insert_start.elapsed();

    let avg_insert_us = insert_elapsed.as_micros() as f64 / n as f64;
    eprintln!(
        "Merkle insert: {} leaves in {:?} (avg {:.1}us/insert)",
        n, insert_elapsed, avg_insert_us
    );

    // Proof generation for the last leaf
    let proof_start = Instant::now();
    let (_path_elements, _path_indices) = tree.get_proof(n - 1).expect("get_proof failed");
    let proof_elapsed = proof_start.elapsed();

    eprintln!(
        "Merkle proof generation: {:?} for leaf {} (depth {})",
        proof_elapsed,
        n - 1,
        depth
    );

    // Target: single insert + proof < 50ms
    let single_op_ms = (avg_insert_us / 1000.0) + proof_elapsed.as_secs_f64() * 1000.0;
    eprintln!(
        "Single insert + proof: {:.2}ms (target < 50ms)",
        single_op_ms
    );
    assert!(
        single_op_ms < 50.0,
        "insert + proof should be under 50ms, got {:.2}ms",
        single_op_ms
    );
}

#[test]
fn perf_merkle_root_computation() {
    let depth = 20;
    let mut tree = MerkleTree::new(depth).expect("failed to create tree");

    // Insert some leaves
    for i in 0..100 {
        tree.insert(Fr::from((i + 1) as u64))
            .expect("insert failed");
    }

    // Time root computation
    let start = Instant::now();
    for _ in 0..1000 {
        let _root = tree.root();
    }
    let elapsed = start.elapsed();

    let avg_ns = elapsed.as_nanos() / 1000;
    eprintln!("Root access: avg {}ns/call", avg_ns);

    // Root should be essentially instant (cached or O(1))
    assert!(
        avg_ns < 1_000_000, // < 1ms per root call
        "root access should be sub-millisecond"
    );
}
