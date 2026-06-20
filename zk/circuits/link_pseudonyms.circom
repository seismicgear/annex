pragma circom 2.0.0;

include "circomlib/circuits/poseidon.circom";

// Link-pseudonyms — voluntarily prove two topic-scoped pseudonyms are the
// SAME identity, without revealing the secret key.
//
// Why this circuit exists:
//   The README states: "Cross-server identity linkage is opt-in via
//   `link-pseudonyms` circuits, never automatic." Until now that circuit did
//   not exist, so there was no cryptographic way for a user to *prove* two of
//   their pseudonyms are the same person. The only linkage mechanism that
//   existed was the server holding plaintext — i.e. the exact involuntary
//   linkage the foundations forbid.
//
//   This circuit lets the HOLDER (and only the holder, who knows `sk`) prove:
//     "nullifierA (my pseudonym in topic A) and nullifierB (my pseudonym in
//      topic B) are both derived from the same secret key" — establishing the
//      link by consent, revealing `sk` to no one.
//
//   Because the v2 nullifier is `Poseidon(sk, topicHash, 1)` and is the value
//   the server already stored in `zk_nullifiers`, this circuit reuses the SAME
//   domain separator (1) so the recomputed nullifiers equal the registered
//   ones. The server verifies the proof, then confirms both nullifiers exist
//   in its (and/or a peer's) nullifier table, and records the consented link.
//
// Public signals (snarkjs ordering = outputs first, then declared public
// inputs): [nullifierA, nullifierB, topicHashA, topicHashB].
//
// See README "Identity Plane: Topic-scoped pseudonyms", FOUNDATIONS §2/§7.

// MUST equal membership_v2.circom's DOMAIN_NULLIFIER_V2 so that the nullifiers
// this circuit recomputes are byte-identical to the ones a v2 membership proof
// emitted and the server persisted. Changing this divorces link-proofs from
// real registered pseudonyms.
template LinkPseudonyms() {
    // Private witness — the single secret tying both pseudonyms together.
    signal input sk;

    // Public inputs — the two topic-hash contexts being linked.
    signal input topicHashA;
    signal input topicHashB;

    // Public outputs — the two nullifiers the circuit recomputes from `sk`.
    signal output nullifierA;
    signal output nullifierB;

    var DOMAIN_NULLIFIER_V2 = 1;

    component nA = Poseidon(3);
    nA.inputs[0] <== sk;
    nA.inputs[1] <== topicHashA;
    nA.inputs[2] <== DOMAIN_NULLIFIER_V2;
    nullifierA <== nA.out;

    component nB = Poseidon(3);
    nB.inputs[0] <== sk;
    nB.inputs[1] <== topicHashB;
    nB.inputs[2] <== DOMAIN_NULLIFIER_V2;
    nullifierB <== nB.out;
}

// Final public-signal ordering: [nullifierA, nullifierB, topicHashA, topicHashB].
component main {public [topicHashA, topicHashB]} = LinkPseudonyms();
