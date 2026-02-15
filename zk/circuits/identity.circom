pragma circom 2.0.0;

include "circomlib/circuits/poseidon.circom";

// Identity Commitment Circuit
// Computes commitment = Poseidon(sk, roleCode, nodeId)
template IdentityCommitment() {
    signal input sk;
    signal input roleCode;
    signal input nodeId;

    signal output commitment;

    component poseidon = Poseidon(3);
    poseidon.inputs[0] <== sk;
    poseidon.inputs[1] <== roleCode;
    poseidon.inputs[2] <== nodeId;

    commitment <== poseidon.out;
}

component main = IdentityCommitment();
