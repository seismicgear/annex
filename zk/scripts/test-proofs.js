const snarkjs = require("snarkjs");
const path = require("path");
const fs = require("fs");
const { buildPoseidon } = require("circomlibjs");

const buildPath = path.resolve(__dirname, "../build");
const keysPath = path.resolve(__dirname, "../keys");

let passed = 0;
let failed = 0;

function assert(condition, msg) {
    if (condition) {
        passed++;
        console.log(`  PASS: ${msg}`);
    } else {
        failed++;
        console.error(`  FAIL: ${msg}`);
    }
}

async function run() {
    const poseidon = await buildPoseidon();
    const idVKey = JSON.parse(fs.readFileSync(path.join(keysPath, "identity_vkey.json")));
    const memVKey = JSON.parse(fs.readFileSync(path.join(keysPath, "membership_vkey.json")));

    // ═══════════════════════════════════════════
    // Identity Circuit — Valid Proof
    // ═══════════════════════════════════════════
    console.log("\n=== Identity Circuit: Valid Proof ===");

    const sk = 123456789n;
    const roleCode = 1n;
    const nodeId = 42n;

    const expectedCommitment = poseidon.F.toString(poseidon([sk, roleCode, nodeId]));

    const { proof: idProof, publicSignals: idSignals } = await snarkjs.groth16.fullProve(
        { sk: sk.toString(), roleCode: roleCode.toString(), nodeId: nodeId.toString() },
        path.join(buildPath, "identity_js/identity.wasm"),
        path.join(keysPath, "identity_final.zkey")
    );

    const idVerified = await snarkjs.groth16.verify(idVKey, idSignals, idProof);
    assert(idVerified, "valid identity proof verifies");
    assert(idSignals[0] === expectedCommitment, "commitment matches expected Poseidon output");

    // ═══════════════════════════════════════════
    // Identity Circuit — Tampered Proof
    // ═══════════════════════════════════════════
    console.log("\n=== Identity Circuit: Tampered Proof ===");

    // Tamper with pi_a to invalidate the proof
    const tamperedIdProof = JSON.parse(JSON.stringify(idProof));
    tamperedIdProof.pi_a[0] = "1"; // Corrupt first coordinate
    const tamperedIdVerified = await snarkjs.groth16.verify(idVKey, idSignals, tamperedIdProof);
    assert(!tamperedIdVerified, "corrupted proof is rejected");

    // Tamper with public signal (claim different commitment)
    const tamperedSignals = [...idSignals];
    tamperedSignals[0] = "12345";
    const mismatchVerified = await snarkjs.groth16.verify(idVKey, tamperedSignals, idProof);
    assert(!mismatchVerified, "proof with tampered public signal is rejected");

    // ═══════════════════════════════════════════
    // Identity Circuit — Different Inputs Produce Different Commitments
    // ═══════════════════════════════════════════
    console.log("\n=== Identity Circuit: Different Inputs ===");

    const { publicSignals: altSignals1 } = await snarkjs.groth16.fullProve(
        { sk: "999999999", roleCode: "1", nodeId: "42" },
        path.join(buildPath, "identity_js/identity.wasm"),
        path.join(keysPath, "identity_final.zkey")
    );
    assert(altSignals1[0] !== idSignals[0], "different sk produces different commitment");

    const { publicSignals: altSignals2 } = await snarkjs.groth16.fullProve(
        { sk: sk.toString(), roleCode: "2", nodeId: "42" },
        path.join(buildPath, "identity_js/identity.wasm"),
        path.join(keysPath, "identity_final.zkey")
    );
    assert(altSignals2[0] !== idSignals[0], "different roleCode produces different commitment");

    const { publicSignals: altSignals3 } = await snarkjs.groth16.fullProve(
        { sk: sk.toString(), roleCode: "1", nodeId: "99" },
        path.join(buildPath, "identity_js/identity.wasm"),
        path.join(keysPath, "identity_final.zkey")
    );
    assert(altSignals3[0] !== idSignals[0], "different nodeId produces different commitment");

    // ═══════════════════════════════════════════
    // Membership Circuit — Valid Proof (leafIndex=0)
    // ═══════════════════════════════════════════
    console.log("\n=== Membership Circuit: Valid Proof (leafIndex=0) ===");

    const depth = 20;
    const pathElements0 = new Array(depth).fill("0");
    const pathIndexBits0 = new Array(depth).fill("0");

    let current = poseidon([sk, roleCode, nodeId]);
    for (let i = 0; i < depth; i++) {
        current = poseidon([current, 0n]);
    }
    const expectedRoot0 = poseidon.F.toString(current);

    const { proof: memProof0, publicSignals: memSignals0 } = await snarkjs.groth16.fullProve(
        {
            sk: sk.toString(), roleCode: roleCode.toString(), nodeId: nodeId.toString(),
            leafIndex: "0", pathElements: pathElements0, pathIndexBits: pathIndexBits0,
        },
        path.join(buildPath, "membership_js/membership.wasm"),
        path.join(keysPath, "membership_final.zkey")
    );

    const memVerified0 = await snarkjs.groth16.verify(memVKey, memSignals0, memProof0);
    assert(memVerified0, "valid membership proof (index 0) verifies");
    assert(memSignals0[0] === expectedRoot0, "root matches expected value");
    assert(memSignals0[1] === expectedCommitment, "commitment matches in membership proof");

    // ═══════════════════════════════════════════
    // Membership Circuit — Valid Proof (leafIndex=1, mixed path bits)
    // ═══════════════════════════════════════════
    console.log("\n=== Membership Circuit: Valid Proof (leafIndex=1) ===");

    // leafIndex=1 means bit[0]=1, rest=0
    const pathIndexBits1 = new Array(depth).fill("0");
    pathIndexBits1[0] = "1";

    // At level 0, our leaf is on the right. Sibling (left) is 0.
    // hash(0, commitment) at level 0; then hash(prev, 0) for levels 1-19
    let current1 = poseidon([0n, poseidon([sk, roleCode, nodeId])]);
    for (let i = 1; i < depth; i++) {
        current1 = poseidon([current1, 0n]);
    }
    const expectedRoot1 = poseidon.F.toString(current1);

    const { proof: memProof1, publicSignals: memSignals1 } = await snarkjs.groth16.fullProve(
        {
            sk: sk.toString(), roleCode: roleCode.toString(), nodeId: nodeId.toString(),
            leafIndex: "1", pathElements: pathElements0, pathIndexBits: pathIndexBits1,
        },
        path.join(buildPath, "membership_js/membership.wasm"),
        path.join(keysPath, "membership_final.zkey")
    );

    const memVerified1 = await snarkjs.groth16.verify(memVKey, memSignals1, memProof1);
    assert(memVerified1, "valid membership proof (index 1) verifies");
    assert(memSignals1[0] === expectedRoot1, "root matches for index 1");

    // ═══════════════════════════════════════════
    // Membership Circuit — Tampered Proof
    // ═══════════════════════════════════════════
    console.log("\n=== Membership Circuit: Tampered Proof ===");

    const tamperedMemProof = JSON.parse(JSON.stringify(memProof0));
    tamperedMemProof.pi_a[0] = "1";
    const tamperedMemVerified = await snarkjs.groth16.verify(memVKey, memSignals0, tamperedMemProof);
    assert(!tamperedMemVerified, "corrupted membership proof is rejected");

    // Tamper with root public signal
    const tamperedMemSignals = [...memSignals0];
    tamperedMemSignals[0] = "99999";
    const rootTamperedVerified = await snarkjs.groth16.verify(memVKey, tamperedMemSignals, memProof0);
    assert(!rootTamperedVerified, "membership proof with tampered root is rejected");

    // Tamper with commitment public signal
    const tamperedCommitmentSignals = [...memSignals0];
    tamperedCommitmentSignals[1] = "99999";
    const commitTamperedVerified = await snarkjs.groth16.verify(memVKey, tamperedCommitmentSignals, memProof0);
    assert(!commitTamperedVerified, "membership proof with tampered commitment is rejected");

    // ═══════════════════════════════════════════
    // Membership Circuit — Wrong Witness (invalid should fail at proof generation)
    // ═══════════════════════════════════════════
    console.log("\n=== Membership Circuit: Mismatched leafIndex vs pathIndexBits ===");

    // leafIndex=0 but pathIndexBits says bit[0]=1 — constraint should fail
    try {
        await snarkjs.groth16.fullProve(
            {
                sk: sk.toString(), roleCode: roleCode.toString(), nodeId: nodeId.toString(),
                leafIndex: "0", pathElements: pathElements0,
                pathIndexBits: ["1", ...new Array(depth - 1).fill("0")],
            },
            path.join(buildPath, "membership_js/membership.wasm"),
            path.join(keysPath, "membership_final.zkey")
        );
        assert(false, "mismatched leafIndex/pathIndexBits should fail witness generation");
    } catch (e) {
        assert(true, "mismatched leafIndex/pathIndexBits rejected at witness generation");
    }

    // ═══════════════════════════════════════════
    // Membership v2 Circuit — secret-derived nullifier
    // ═══════════════════════════════════════════
    const memV2VKey = JSON.parse(
        fs.readFileSync(path.join(keysPath, "membership_v2_vkey.json")),
    );
    const v2Wasm = path.join(buildPath, "membership_v2_js/membership_v2.wasm");
    const v2Zkey = path.join(keysPath, "membership_v2_final.zkey");

    // The verifier supplies topicHash. In the live protocol it would be
    // Poseidon of the canonicalised topic string; for testing we just pick
    // arbitrary field elements.
    const topicHashA = 7777777777777777n;
    const topicHashB = 8888888888888888n;
    const DOMAIN_NULLIFIER_V2 = 1n;

    function expectedNullifier(skVal, topicVal) {
        return poseidon.F.toString(
            poseidon([skVal, topicVal, DOMAIN_NULLIFIER_V2]),
        );
    }

    console.log("\n=== Membership v2: Valid Proof ===");

    const v2InputA = {
        sk: sk.toString(),
        roleCode: roleCode.toString(),
        nodeId: nodeId.toString(),
        leafIndex: "0",
        pathElements: pathElements0,
        pathIndexBits: pathIndexBits0,
        topicHash: topicHashA.toString(),
    };
    const { proof: v2ProofA, publicSignals: v2SignalsA } =
        await snarkjs.groth16.fullProve(v2InputA, v2Wasm, v2Zkey);

    // Public signals layout: [root, commitment, nullifier, topicHash].
    assert(v2SignalsA.length === 4, "v2 publicSignals.length === 4");
    const v2VerifiedA = await snarkjs.groth16.verify(
        memV2VKey,
        v2SignalsA,
        v2ProofA,
    );
    assert(v2VerifiedA, "valid v2 proof verifies");
    assert(v2SignalsA[0] === expectedRoot0, "v2 root matches expected");
    assert(
        v2SignalsA[1] === expectedCommitment,
        "v2 commitment matches expected",
    );
    assert(
        v2SignalsA[2] === expectedNullifier(sk, topicHashA),
        "v2 nullifier = Poseidon(sk, topicHash, DOMAIN_NULLIFIER_V2)",
    );
    assert(
        v2SignalsA[3] === topicHashA.toString(),
        "v2 publicSignals[3] echoes topicHash",
    );

    console.log("\n=== Membership v2: Tampered Nullifier ===");

    const tamperedNullifierSignals = [...v2SignalsA];
    tamperedNullifierSignals[2] = "12345"; // wrong nullifier
    const tamperedNullifierVerified = await snarkjs.groth16.verify(
        memV2VKey,
        tamperedNullifierSignals,
        v2ProofA,
    );
    assert(
        !tamperedNullifierVerified,
        "v2 proof with tampered nullifier is rejected",
    );

    console.log("\n=== Membership v2: Tampered topicHash ===");

    const tamperedTopicSignals = [...v2SignalsA];
    tamperedTopicSignals[3] = topicHashB.toString(); // claim a different topic
    const tamperedTopicVerified = await snarkjs.groth16.verify(
        memV2VKey,
        tamperedTopicSignals,
        v2ProofA,
    );
    assert(
        !tamperedTopicVerified,
        "v2 proof with tampered topicHash is rejected",
    );

    console.log("\n=== Membership v2: Mismatched leafIndex vs pathIndexBits ===");

    try {
        await snarkjs.groth16.fullProve(
            {
                ...v2InputA,
                leafIndex: "0",
                pathIndexBits: ["1", ...new Array(depth - 1).fill("0")],
            },
            v2Wasm,
            v2Zkey,
        );
        assert(
            false,
            "v2 mismatched leafIndex/pathIndexBits should fail witness generation",
        );
    } catch (e) {
        assert(
            true,
            "v2 mismatched leafIndex/pathIndexBits rejected at witness generation",
        );
    }

    console.log("\n=== Membership v2: Same sk + same topic => same nullifier ===");

    // Prove again with identical witness — nullifier MUST match exactly.
    const { publicSignals: v2SignalsAReplay } = await snarkjs.groth16.fullProve(
        v2InputA,
        v2Wasm,
        v2Zkey,
    );
    assert(
        v2SignalsAReplay[2] === v2SignalsA[2],
        "same sk + same topicHash produces the same nullifier (deterministic)",
    );

    console.log(
        "\n=== Membership v2: Same sk + different topic => different nullifier ===",
    );

    const v2InputB = { ...v2InputA, topicHash: topicHashB.toString() };
    const { proof: v2ProofB, publicSignals: v2SignalsB } =
        await snarkjs.groth16.fullProve(v2InputB, v2Wasm, v2Zkey);
    const v2VerifiedB = await snarkjs.groth16.verify(
        memV2VKey,
        v2SignalsB,
        v2ProofB,
    );
    assert(v2VerifiedB, "v2 proof for different topic verifies");
    assert(
        v2SignalsB[2] !== v2SignalsA[2],
        "same sk + different topicHash produces a different nullifier",
    );
    assert(
        v2SignalsB[2] === expectedNullifier(sk, topicHashB),
        "v2 nullifier for second topic matches Poseidon(sk, topicHashB, DOMAIN)",
    );

    // ═══════════════════════════════════════════
    // Summary
    // ═══════════════════════════════════════════
    console.log(`\n=== Results: ${passed} passed, ${failed} failed ===`);
    if (failed > 0) {
        process.exit(1);
    }
}

run().then(() => {
    console.log("\nAll tests passed.");
    process.exit(0);
}).catch(err => {
    console.error("Test failed:", err);
    process.exit(1);
});
