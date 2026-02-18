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
