pragma circom 2.0.0;

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";

// Membership v2 — secret-derived nullifier.
//
// Why v2:
//   v1 derives the per-topic nullifier as `sha256(commitmentHex + ":" + topic)`
//   on the client. The commitment is a public Merkle leaf, so anyone with
//   read access to the registry (federation peers, a leaked snapshot, an
//   ex-operator) can compute every pseudonym for every topic. That is not
//   zero-knowledge: it is a public deterministic mapping from leaf -> handle.
//
//   v2 binds the nullifier to the holder's secret key inside the circuit so
//   that observing the commitment does not let an attacker compute the
//   topic-scoped pseudonym. Knowledge of `sk` is required.
//
// Public signals (snarkjs ordering = outputs first, then declared public
// inputs): [root, commitment, nullifier, topicHash].
//
// Migration: this is a NEW circuit shipped alongside v1. v1 stays in place
// until every client has been updated. The Rust verifier dispatches by an
// explicit `protocol_version` field; the server never silently mixes v1
// and v2 semantics.
//
// See docs/refactor/zk-merkle-production.md.

// Re-include the Merkle inclusion check from v1. We deliberately copy
// rather than `include` v1 so v2 has its own self-contained source of
// truth and can be re-audited without dragging v1 along.
template MerkleTreeInclusionProofV2(depth) {
    signal input leaf;
    signal input pathElements[depth];
    signal input pathIndexBits[depth];
    signal output root;

    component poseidons[depth];

    signal currentHash[depth + 1];
    currentHash[0] <== leaf;

    for (var i = 0; i < depth; i++) {
        poseidons[i] = Poseidon(2);
        var left = pathIndexBits[i] * (pathElements[i] - currentHash[i]) + currentHash[i];
        var right = pathIndexBits[i] * (currentHash[i] - pathElements[i]) + pathElements[i];
        poseidons[i].inputs[0] <== left;
        poseidons[i].inputs[1] <== right;
        currentHash[i+1] <== poseidons[i].out;
    }

    root <== currentHash[depth];
}

// MembershipV2(depth)
//
// Private inputs:
//   sk             — holder's secret scalar (BN254 Fr).
//   roleCode       — agent / human / service role.
//   nodeId         — local node identifier within the holder's deployment.
//   leafIndex      — index of the holder's leaf in the depth-`depth` tree.
//   pathElements[] — sibling hashes along the Merkle path to the root.
//   pathIndexBits[]— direction bits along the same path.
//
// Public input:
//   topicHash      — Poseidon hash of the topic context, supplied by the
//                    verifier (not the prover) to prevent topic substitution.
//
// Public outputs:
//   root           — the Merkle root the inclusion proof matches.
//   commitment     — Poseidon(sk, roleCode, nodeId), the holder's leaf.
//   nullifier      — Poseidon(sk, topicHash, DOMAIN_NULLIFIER_V2). Same
//                    sk + topicHash always yields the same nullifier (so the
//                    server can detect double-joins). Different sk OR
//                    different topicHash yields a different nullifier.
template MembershipV2(depth) {
    // Private witness.
    signal input sk;
    signal input roleCode;
    signal input nodeId;
    signal input leafIndex;
    signal input pathElements[depth];
    signal input pathIndexBits[depth];

    // Public input — the topic context for which this proof is valid.
    signal input topicHash;

    // Public outputs.
    signal output root;
    signal output commitment;
    signal output nullifier;

    // Domain separator distinguishing nullifier hashes from commitment
    // hashes inside the same Poseidon family. Must stay constant across
    // all v2 deployments — changing it invalidates every previously-
    // emitted v2 nullifier and is therefore a hard wire-format break.
    var DOMAIN_NULLIFIER_V2 = 1;

    // 1. Recompute Identity Commitment.
    component identity = Poseidon(3);
    identity.inputs[0] <== sk;
    identity.inputs[1] <== roleCode;
    identity.inputs[2] <== nodeId;
    commitment <== identity.out;

    // 2. Verify Merkle Path against the recomputed commitment.
    component merkleProof = MerkleTreeInclusionProofV2(depth);
    merkleProof.leaf <== commitment;
    for (var i = 0; i < depth; i++) {
        merkleProof.pathElements[i] <== pathElements[i];
        merkleProof.pathIndexBits[i] <== pathIndexBits[i];
    }
    root <== merkleProof.root;

    // 3. Constrain leafIndex bits to match pathIndexBits — same invariant
    //    as v1: prevents a prover from claiming an index that doesn't
    //    correspond to the path actually walked.
    component num2Bits = Num2Bits(depth);
    num2Bits.in <== leafIndex;
    for (var i = 0; i < depth; i++) {
        num2Bits.out[i] === pathIndexBits[i];
    }

    // 4. Secret-derived nullifier. The use of `sk` here is the entire
    //    point of v2: the nullifier cannot be computed from public
    //    information alone.
    component nullifierHash = Poseidon(3);
    nullifierHash.inputs[0] <== sk;
    nullifierHash.inputs[1] <== topicHash;
    nullifierHash.inputs[2] <== DOMAIN_NULLIFIER_V2;
    nullifier <== nullifierHash.out;
}

// `topicHash` is declared `public` so snarkjs lays it after the outputs in
// the public-signal vector. Final ordering: [root, commitment, nullifier, topicHash].
component main {public [topicHash]} = MembershipV2(20);
