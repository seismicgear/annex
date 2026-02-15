const snarkjs = require("snarkjs");
const path = require("path");
const fs = require("fs");
const { buildPoseidon } = require("circomlibjs");

const buildPath = path.resolve(__dirname, "../build");
const keysPath = path.resolve(__dirname, "../keys");

async function run() {
    const poseidon = await buildPoseidon();

    // --- Test Identity Circuit ---
    console.log("Testing Identity Circuit...");

    const sk = 123456789n;
    const roleCode = 1n;
    const nodeId = 42n;

    const expectedCommitment = poseidon.F.toString(poseidon([sk, roleCode, nodeId]));
    console.log("Expected Commitment:", expectedCommitment);

    const identityInputs = {
        sk: sk.toString(),
        roleCode: roleCode.toString(),
        nodeId: nodeId.toString()
    };

    const { proof: idProof, publicSignals: idSignals } = await snarkjs.groth16.fullProve(
        identityInputs,
        path.join(buildPath, "identity_js/identity.wasm"),
        path.join(keysPath, "identity_final.zkey")
    );

    const idVKey = JSON.parse(fs.readFileSync(path.join(keysPath, "identity_vkey.json")));
    const idVerified = await snarkjs.groth16.verify(idVKey, idSignals, idProof);

    if (idVerified) {
        console.log("Identity Proof Verified!");
    } else {
        console.error("Identity Proof Verification Failed");
        process.exit(1);
    }

    if (idSignals[0] === expectedCommitment) {
        console.log("Commitment matches expected value.");
    } else {
        console.error(`Commitment Mismatch: Got ${idSignals[0]}, Expected ${expectedCommitment}`);
        process.exit(1);
    }

    // --- Test Membership Circuit ---
    console.log("\nTesting Membership Circuit...");

    // Construct a fake Merkle path for depth 20
    const depth = 20;
    const leafIndex = 0; // Index 0 -> all path bits 0
    const pathElements = new Array(depth).fill(0n);
    const pathIndexBits = new Array(depth).fill(0);

    // Calculate expected root manually
    // Since all siblings are 0 and leaf is commitment, and all bits are 0 (left):
    // level 0: hash(commitment, 0)
    // level 1: hash(prev, 0)
    // ...
    let current = poseidon([sk, roleCode, nodeId]);
    for (let i = 0; i < depth; i++) {
        // Left child (current), right child (0)
        current = poseidon([current, 0n]);
    }
    const expectedRoot = poseidon.F.toString(current);
    console.log("Expected Root:", expectedRoot);

    const membershipInputs = {
        sk: sk.toString(),
        roleCode: roleCode.toString(),
        nodeId: nodeId.toString(),
        leafIndex: leafIndex.toString(),
        pathElements: pathElements.map(e => e.toString()),
        pathIndexBits: pathIndexBits.map(b => b.toString())
    };

    const { proof: memProof, publicSignals: memSignals } = await snarkjs.groth16.fullProve(
        membershipInputs,
        path.join(buildPath, "membership_js/membership.wasm"),
        path.join(keysPath, "membership_final.zkey")
    );

    const memVKey = JSON.parse(fs.readFileSync(path.join(keysPath, "membership_vkey.json")));
    const memVerified = await snarkjs.groth16.verify(memVKey, memSignals, memProof);

    if (memVerified) {
        console.log("Membership Proof Verified!");
    } else {
        console.error("Membership Proof Verification Failed");
        process.exit(1);
    }

    // Signal order depends on circuit definition.
    // Membership template output: root, commitment.
    // So publicSignals[0] should be root, [1] should be commitment.
    if (memSignals[0] === expectedRoot) {
        console.log("Root matches expected value.");
    } else {
        console.error(`Root Mismatch: Got ${memSignals[0]}, Expected ${expectedRoot}`);
        process.exit(1);
    }
}

run().then(() => {
    console.log("All tests passed.");
    process.exit(0);
}).catch(err => {
    console.error("Test failed:", err);
    process.exit(1);
});
