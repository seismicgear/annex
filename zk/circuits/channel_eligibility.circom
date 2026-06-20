pragma circom 2.0.0;

include "circomlib/circuits/poseidon.circom";
include "circomlib/circuits/bitify.circom";

// Channel-eligibility — prove role-gated channel access WITHOUT revealing identity.
//
// Why this circuit exists:
//   The README advertises "channel access requires a valid Groth16 proof" and
//   the FOUNDATIONS doc forbids "security theater" — yet channel capability
//   gating historically read a plaintext role flag out of `channel_service.rs`.
//   A server holding the plaintext leaf↔role mapping can deanonymise every
//   participant of every gated channel. That is exactly the deanonymisation
//   Annex promises never happens.
//
//   This circuit proves, in zero knowledge:
//     "I am a member of the server's Merkle tree under `root`, AND the role
//      bound into my identity commitment equals the role this channel requires,
//      AND here is a channel-scoped nullifier so the server can deduplicate me
//      within this channel" — all WITHOUT revealing which leaf I am.
//
//   The role the channel requires (`requiredRoleCode`) is public — the channel
//   policy is not a secret. What stays hidden is *which* member is presenting
//   the proof. The server learns "some member with role X joined", never "leaf
//   #4172 joined".
//
// Public signals (snarkjs ordering = outputs first, then declared public
// inputs): [root, nullifier, requiredRoleCode, channelTopicHash].
//
// Migration: NEW circuit shipped alongside membership v1/v2. The server
// dispatches by an explicit circuit identifier; it never silently mixes this
// with the membership verifiers (different public-signal arity).
//
// See README "Identity Plane", FOUNDATIONS §6, AUDIT P4-ID-1.

template MerkleTreeInclusionProofElig(depth) {
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

// ChannelEligibility(depth)
//
// Private inputs:
//   sk             — holder's secret scalar (BN254 Fr).
//   roleCode       — the role bound into the holder's identity commitment.
//   nodeId         — local node identifier within the holder's deployment.
//   leafIndex      — index of the holder's leaf in the depth-`depth` tree.
//   pathElements[] — sibling hashes along the Merkle path to the root.
//   pathIndexBits[]— direction bits along the same path.
//
// Public inputs:
//   requiredRoleCode — the role this channel admits. The circuit enforces the
//                      holder's hidden `roleCode` equals it.
//   channelTopicHash — Poseidon/field encoding of the channel's VRP topic,
//                      supplied by the verifier to bind the nullifier to THIS
//                      channel (prevents cross-channel nullifier reuse).
//
// Public outputs:
//   root           — the Merkle root the inclusion proof matches.
//   nullifier      — Poseidon(sk, channelTopicHash, DOMAIN_ELIGIBILITY). Same
//                    sk + channel → same nullifier (per-channel dedupe);
//                    different channel → unlinkable nullifier.
template ChannelEligibility(depth) {
    // Private witness.
    signal input sk;
    signal input roleCode;
    signal input nodeId;
    signal input leafIndex;
    signal input pathElements[depth];
    signal input pathIndexBits[depth];

    // Public inputs.
    signal input requiredRoleCode;
    signal input channelTopicHash;

    // Public outputs.
    signal output root;
    signal output nullifier;

    // Domain separator. MUST differ from membership-v2 (1) and
    // federation-attestation (3) so an eligibility nullifier can never be
    // replayed as a membership or attestation nullifier.
    var DOMAIN_ELIGIBILITY = 2;

    // 1. Recompute identity commitment from the hidden witness.
    component identity = Poseidon(3);
    identity.inputs[0] <== sk;
    identity.inputs[1] <== roleCode;
    identity.inputs[2] <== nodeId;
    signal commitment;
    commitment <== identity.out;

    // 2. Prove the commitment is a leaf under `root`.
    component merkleProof = MerkleTreeInclusionProofElig(depth);
    merkleProof.leaf <== commitment;
    for (var i = 0; i < depth; i++) {
        merkleProof.pathElements[i] <== pathElements[i];
        merkleProof.pathIndexBits[i] <== pathIndexBits[i];
    }
    root <== merkleProof.root;

    // 3. Bind leafIndex bits to the path actually walked.
    component num2Bits = Num2Bits(depth);
    num2Bits.in <== leafIndex;
    for (var i = 0; i < depth; i++) {
        num2Bits.out[i] === pathIndexBits[i];
    }

    // 4. The eligibility predicate: the hidden role must equal the role the
    //    channel admits. This is a hard equality constraint — a member whose
    //    role differs cannot produce a satisfying witness.
    roleCode === requiredRoleCode;

    // 5. Channel-scoped, secret-derived nullifier (same secrecy property as
    //    membership v2: not computable from the public commitment alone).
    component nullifierHash = Poseidon(3);
    nullifierHash.inputs[0] <== sk;
    nullifierHash.inputs[1] <== channelTopicHash;
    nullifierHash.inputs[2] <== DOMAIN_ELIGIBILITY;
    nullifier <== nullifierHash.out;
}

// Final public-signal ordering: [root, nullifier, requiredRoleCode, channelTopicHash].
component main {public [requiredRoleCode, channelTopicHash]} = ChannelEligibility(20);
