pragma circom 2.0.0;

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";

// Federation-attestation — Server B proves to Server A that *some* member of
// B's registry is present in a federation context, WITHOUT revealing which
// member, and WITHOUT Server A ever seeing B's identity database.
//
// Why this circuit exists:
//   The README "Federation Plane" promises: "Server B proves membership of its
//   users to Server A via signed Merkle root exchange and Groth16 proof
//   verification ... At no point does Server A need Server B's raw identity
//   database." Until now that flow had NO zero-knowledge backing — federated
//   attestation was a metadata record, so the privacy claim ("Server A never
//   sees who") was unbacked.
//
//   This circuit produces a portable attestation:
//     "I (the holder) am a member under Server B's published `root`, and here
//      is a federation-context-scoped nullifier" — verifiable by Server A
//      against B's published root, revealing neither `sk` nor the leaf index.
//
//   The `federationContextHash` binds the attestation to a specific federation
//   agreement / context so a nullifier minted for one federation relationship
//   cannot be replayed into another. The nullifier lets Server A dedupe the
//   attesting member within that context without learning their identity.
//
// Public signals (snarkjs ordering = outputs first, then declared public
// inputs): [root, nullifier, federationContextHash].
//
// See README "Federation Plane: Cross-instance VRP attestation", FOUNDATIONS §3.

template MerkleTreeInclusionProofFed(depth) {
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

// FederationAttestation(depth)
//
// Private inputs:
//   sk, roleCode, nodeId — the holder's identity-commitment preimage.
//   leafIndex, pathElements[], pathIndexBits[] — Merkle inclusion witness.
//
// Public input:
//   federationContextHash — field encoding of the federation context (e.g.
//     hash of the agreement id / peer slug pair), supplied by the verifier.
//
// Public outputs:
//   root      — Server B's Merkle root the inclusion proof matches.
//   nullifier — Poseidon(sk, federationContextHash, DOMAIN_FEDERATION).
template FederationAttestation(depth) {
    signal input sk;
    signal input roleCode;
    signal input nodeId;
    signal input leafIndex;
    signal input pathElements[depth];
    signal input pathIndexBits[depth];

    signal input federationContextHash;

    signal output root;
    signal output nullifier;

    // Domain separator. MUST differ from membership-v2 (1) and
    // channel-eligibility (2).
    var DOMAIN_FEDERATION = 3;

    // 1. Recompute identity commitment.
    component identity = Poseidon(3);
    identity.inputs[0] <== sk;
    identity.inputs[1] <== roleCode;
    identity.inputs[2] <== nodeId;
    signal commitment;
    commitment <== identity.out;

    // 2. Merkle inclusion under the published root.
    component merkleProof = MerkleTreeInclusionProofFed(depth);
    merkleProof.leaf <== commitment;
    for (var i = 0; i < depth; i++) {
        merkleProof.pathElements[i] <== pathElements[i];
        merkleProof.pathIndexBits[i] <== pathIndexBits[i];
    }
    root <== merkleProof.root;

    // 3. Bind leafIndex bits to the walked path.
    component num2Bits = Num2Bits(depth);
    num2Bits.in <== leafIndex;
    for (var i = 0; i < depth; i++) {
        num2Bits.out[i] === pathIndexBits[i];
    }

    // 4. Federation-context-scoped, secret-derived nullifier.
    component nullifierHash = Poseidon(3);
    nullifierHash.inputs[0] <== sk;
    nullifierHash.inputs[1] <== federationContextHash;
    nullifierHash.inputs[2] <== DOMAIN_FEDERATION;
    nullifier <== nullifierHash.out;
}

// Final public-signal ordering: [root, nullifier, federationContextHash].
component main {public [federationContextHash]} = FederationAttestation(20);
