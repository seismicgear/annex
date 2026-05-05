#!/usr/bin/env node
// dev-setup-groth16.js — Local dev-only Groth16 setup with random entropy.
//
// PRODUCTION USE IS FORBIDDEN. This script generates a Powers-of-Tau
// contribution and per-circuit zkey contribution using `crypto.randomBytes`
// on a single machine. The resulting artifacts are NOT from a multi-party
// ceremony and any zkey produced here is unsafe for any deployment that
// claims production-grade ZK.
//
// Production builds must instead consume pinned artifacts whose hashes are
// recorded in zk/artifacts/<circuit>/manifest.json and verified at build
// time by zk/scripts/verify-artifacts.js. See
// docs/refactor/zk-merkle-production.md.
//
// Refuses to run when ANNEX_BUILD_PROFILE=production. Lower-cased for safety.

"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { execSync } = require("child_process");

const profile = (process.env.ANNEX_BUILD_PROFILE || "").trim().toLowerCase();
if (profile === "production" || profile === "release") {
  process.stderr.write(
    "[dev-setup-groth16] REFUSING to run: ANNEX_BUILD_PROFILE=" +
      process.env.ANNEX_BUILD_PROFILE +
      ".\n" +
      "[dev-setup-groth16] Production builds must use pre-pinned artifacts;\n" +
      "[dev-setup-groth16] verify them via `node zk/scripts/verify-artifacts.js`.\n"
  );
  process.exit(1);
}

process.stdout.write(
  "[dev-setup-groth16] DEV-ONLY: generating Groth16 keys with random entropy.\n" +
    "[dev-setup-groth16] Output is NOT safe for production. Replace with real-ceremony\n" +
    "[dev-setup-groth16] artifacts before tagging a public release.\n"
);

const buildPath = path.resolve(__dirname, "../build");
const keysPath = path.resolve(__dirname, "../keys");

/// Generates a cryptographically random hex string for ceremony entropy.
/// SECURITY: Using hardcoded strings (e.g., "random text") as ceremony
/// entropy completely compromises the trusted setup — any adversary knowing
/// the entropy can forge valid proofs for arbitrary statements.
function randomEntropy() {
    return crypto.randomBytes(32).toString("hex");
}

if (!fs.existsSync(keysPath)) {
    fs.mkdirSync(keysPath);
}

const circuits = ['identity', 'membership', 'membership_v2'];

function run(cmd) {
    console.log(`Running: ${cmd}`);
    execSync(cmd, { stdio: 'inherit', cwd: path.resolve(__dirname, "..") });
}

async function setup() {
    const ptauPath = path.join(keysPath, "pot14_final.ptau");
    const ptau0 = path.join(keysPath, "pot14_0000.ptau");
    const ptau1 = path.join(keysPath, "pot14_0001.ptau");

    if (!fs.existsSync(ptauPath)) {
        console.log("Generating Powers of Tau...");
        // 1. Start a new powers of tau ceremony
        run(`npx snarkjs powersoftau new bn128 14 "${ptau0}" -v`);
        // 2. Contribute to the ceremony
        run(`npx snarkjs powersoftau contribute "${ptau0}" "${ptau1}" --name="First Contribution" -v -e="${randomEntropy()}"`);
        // 3. Prepare for phase 2
        run(`npx snarkjs powersoftau prepare phase2 "${ptau1}" "${ptauPath}" -v`);
    }

    for (const circuit of circuits) {
        console.log(`Setting up ${circuit}...`);
        const r1csPath = path.join(buildPath, `${circuit}.r1cs`);
        const zkey0 = path.join(keysPath, `${circuit}_0.zkey`);
        const zkeyFinal = path.join(keysPath, `${circuit}_final.zkey`);
        const vkeyPath = path.join(keysPath, `${circuit}_vkey.json`);

        // 4. Setup Phase 2
        run(`npx snarkjs groth16 setup "${r1csPath}" "${ptauPath}" "${zkey0}"`);

        // 5. Contribute to Phase 2
        run(`npx snarkjs zkey contribute "${zkey0}" "${zkeyFinal}" --name="Second Contribution" -v -e="${randomEntropy()}"`);

        // 6. Export verification key
        run(`npx snarkjs zkey export verificationkey "${zkeyFinal}" "${vkeyPath}"`);

        console.log(`${circuit} setup complete.`);
    }
}

setup().catch(err => {
    console.error(err);
    process.exit(1);
});
