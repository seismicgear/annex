pragma circom 2.0.0;

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";

// Merkle Tree Inclusion Proof
// Verifies that a leaf exists in a Merkle tree at a given index
template MerkleTreeInclusionProof(depth) {
    signal input leaf;
    signal input pathElements[depth];
    signal input pathIndexBits[depth];
    signal output root;

    component poseidons[depth];
    component mux[depth];

    signal currentHash[depth + 1];
    currentHash[0] <== leaf;

    for (var i = 0; i < depth; i++) {
        poseidons[i] = Poseidon(2);

        // Path index bit: 0 = left, 1 = right
        // If 0: hash(current, pathElement)
        // If 1: hash(pathElement, current)

        // We can use a mathematical trick or a Mux.
        // Left input = pathIndexBit * (pathElement - current) + current
        // Right input = pathIndexBit * (current - pathElement) + pathElement

        var left = pathIndexBits[i] * (pathElements[i] - currentHash[i]) + currentHash[i];
        var right = pathIndexBits[i] * (currentHash[i] - pathElements[i]) + pathElements[i];

        poseidons[i].inputs[0] <== left;
        poseidons[i].inputs[1] <== right;

        currentHash[i+1] <== poseidons[i].out;
    }

    root <== currentHash[depth];
}

// Membership Circuit
// Proves ownership of an identity commitment included in the Merkle tree
template Membership(depth) {
    signal input sk;
    signal input roleCode;
    signal input nodeId;

    signal input leafIndex;
    signal input pathElements[depth];
    signal input pathIndexBits[depth];

    signal output root;
    signal output commitment;

    // 1. Recompute Identity Commitment
    component identity = Poseidon(3);
    identity.inputs[0] <== sk;
    identity.inputs[1] <== roleCode;
    identity.inputs[2] <== nodeId;

    commitment <== identity.out;

    // 2. Verify Merkle Path
    component merkleProof = MerkleTreeInclusionProof(depth);
    merkleProof.leaf <== commitment;

    for (var i = 0; i < depth; i++) {
        merkleProof.pathElements[i] <== pathElements[i];
        merkleProof.pathIndexBits[i] <== pathIndexBits[i];
    }

    root <== merkleProof.root;

    // 3. Constrain leafIndex bits to match pathIndexBits
    // This ensures the proof structure matches the claimed index
    component num2Bits = Num2Bits(depth);
    num2Bits.in <== leafIndex;

    for (var i = 0; i < depth; i++) {
        // pathIndexBits[0] is the bottom-most level (leaf level)
        // Num2Bits outputs little-endian bits (index 0 is LSB)
        // In a binary tree, LSB determines left/right at the bottom.
        // So they should match directly.
        num2Bits.out[i] === pathIndexBits[i];
    }
}

component main = Membership(20);
