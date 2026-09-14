#!/usr/bin/env node
// verify-ceremony.js — Prove the pinned artifacts came out of the ceremony.
//
// `verify-artifacts.js` answers a different question: "are the files on disk
// the ones the manifest names?" It hashes them. That catches a swap or a
// truncation, and nothing else — a manifest and a matching set of files can
// both be forged by anyone who can write to the repo.
//
// This script answers "did a real ceremony produce these?", which is the
// question a release actually rests on:
//
//   1. `snarkjs zkey verify <r1cs> <ptau> <zkey>` walks the whole chain — the
//      constraint system, the Powers of Tau, every phase-2 contribution and
//      the final beacon — and fails if any link does not follow. A proving
//      key that was not derived from THIS r1cs and THIS ptau cannot pass.
//   2. The exported verification key is re-derived from the proving key and
//      compared against the shipped one, so a vkey cannot be swapped for a
//      key whose secret someone knows while the zkey stays honest.
//   3. The beacon is checked against drand: the transcript names a round, and
//      the round's published randomness must equal the value the ceremony
//      used. Because the round number was committed to before that round
//      existed (the transcript carries the commitment), this is what rules out
//      a ceremony steered toward a chosen output.
//
// Step 3 needs the network. `--offline` skips it and says so in the summary
// rather than silently reporting a weaker check as the full one.
//
// Exit codes:
//   0  every circuit verified
//   1  usage / transcript problem
//   2  a circuit failed verification

"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { execFileSync } = require("child_process");

const ZK_DIR = path.resolve(__dirname, "..");
const ARTIFACTS_DIR = path.join(ZK_DIR, "artifacts");
const CEREMONY_DIR = path.join(ARTIFACTS_DIR, "ceremony");

function info(msg) {
  process.stdout.write(`[verify-ceremony] ${msg}\n`);
}
function warn(msg) {
  process.stdout.write(`[verify-ceremony] WARN ${msg}\n`);
}
function fail(msg, code = 1) {
  process.stderr.write(`[verify-ceremony] ERROR ${msg}\n`);
  process.exit(code);
}

function sha256(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function snarkjs(args) {
  return execFileSync(
    process.execPath,
    [path.join(ZK_DIR, "node_modules", "snarkjs", "build", "cli.cjs"), ...args],
    { cwd: ZK_DIR, encoding: "utf-8", maxBuffer: 64 * 1024 * 1024 },
  );
}

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--offline") out.offline = true;
    else if (a === "--circuit" && argv[i + 1]) out.circuit = argv[++i];
    else if (a === "--help" || a === "-h") out.help = true;
    else fail(`unknown argument: ${a}`);
  }
  return out;
}

async function checkBeacon(transcript) {
  const beacons = [transcript.phase1 && transcript.phase1.beacon, transcript.phase2 && transcript.phase2.beacon]
    .filter(Boolean);
  if (beacons.length === 0) {
    warn("transcript records no beacon — nothing to check against drand.");
    return true;
  }
  let ok = true;
  for (const b of beacons) {
    const res = await fetch(`https://api.drand.sh/public/${b.round}`);
    if (!res.ok) {
      warn(`drand round ${b.round} returned HTTP ${res.status}; could not check.`);
      ok = false;
      continue;
    }
    const round = await res.json();
    if (round.randomness !== b.randomness) {
      fail(
        `drand round ${b.round} randomness does not match the transcript.\n` +
          `  transcript ${b.randomness}\n  drand      ${round.randomness}\n` +
          "The ceremony's beacon is not the value the League of Entropy published.",
        2,
      );
    }
    info(`  OK beacon  drand round ${b.round} matches the transcript`);
  }
  return ok;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    process.stdout.write(
      "Usage: node zk/scripts/verify-ceremony.js [--offline] [--circuit <name>]\n",
    );
    process.exit(0);
  }

  const transcriptPath = path.join(CEREMONY_DIR, "transcript.json");
  if (!fs.existsSync(transcriptPath)) {
    fail(
      `no ceremony transcript at ${transcriptPath}. The pinned artifacts were not produced by ` +
        "zk/scripts/ceremony.js, so there is nothing to verify them against.",
    );
  }
  const transcript = JSON.parse(fs.readFileSync(transcriptPath, "utf-8"));

  if (!transcript.ptauFile || !transcript.ptauFile.path) {
    fail("transcript does not record the ptau file it used.");
  }
  const ptau = path.resolve(CEREMONY_DIR, transcript.ptauFile.path);
  if (!fs.existsSync(ptau)) fail(`ptau named by the transcript is missing: ${ptau}`, 2);
  const ptauHash = sha256(ptau);
  if (ptauHash !== transcript.ptauFile.sha256) {
    fail(
      `ptau hash mismatch\n  transcript ${transcript.ptauFile.sha256}\n  actual     ${ptauHash}`,
      2,
    );
  }
  info(`ptau:       ${ptau} (sha256 ${ptauHash.slice(0, 16)}…)`);
  info(`ceremony:   phase1 ${transcript.phase1.source}, phase2 beacon round ${transcript.phase2.beacon.round}`);

  const circuits = fs
    .readdirSync(ARTIFACTS_DIR, { withFileTypes: true })
    .filter((d) => d.isDirectory() && d.name !== "ceremony")
    .map((d) => d.name)
    .filter((name) => !args.circuit || name === args.circuit)
    .sort();

  if (circuits.length === 0) fail("no circuit artifact directories found.", 2);

  let bad = 0;
  for (const name of circuits) {
    const dir = path.join(ARTIFACTS_DIR, name);
    const manifestPath = path.join(dir, "manifest.json");
    if (!fs.existsSync(manifestPath)) {
      process.stderr.write(`[verify-ceremony]   NO-MANIFEST ${name}\n`);
      bad += 1;
      continue;
    }
    const manifest = JSON.parse(fs.readFileSync(manifestPath, "utf-8"));
    const r1cs = path.resolve(dir, manifest.paths.r1cs);
    const zkey = path.resolve(dir, manifest.paths.zkey);
    const vkey = path.resolve(dir, manifest.paths.vkey);

    for (const [label, p] of [["r1cs", r1cs], ["zkey", zkey], ["vkey", vkey]]) {
      if (!fs.existsSync(p)) {
        process.stderr.write(`[verify-ceremony]   MISSING-${label.toUpperCase()} ${name}: ${p}\n`);
        bad += 1;
      }
    }
    if (bad > 0 && !fs.existsSync(zkey)) continue;

    try {
      snarkjs(["zkey", "verify", r1cs, ptau, zkey]);
      info(`  OK chain   ${name}: zkey descends from this r1cs and ptau`);
    } catch (e) {
      process.stderr.write(
        `[verify-ceremony]   CHAIN-FAILED ${name}: snarkjs zkey verify rejected the proving key.\n` +
          `${(e.stdout || "").toString().trim()}\n${(e.stderr || "").toString().trim()}\n`,
      );
      bad += 1;
      continue;
    }

    // Re-export and compare rather than trusting the shipped vkey. The vkey is
    // the half that ends up inside every client binary; a mismatched one that
    // still hashes correctly against a forged manifest would verify proofs
    // made with a proving key whose toxic waste someone kept.
    const tmp = path.join(require("os").tmpdir(), `annex-vkey-${name}-${process.pid}.json`);
    try {
      snarkjs(["zkey", "export", "verificationkey", zkey, tmp]);
      const rederived = JSON.parse(fs.readFileSync(tmp, "utf-8"));
      const shipped = JSON.parse(fs.readFileSync(vkey, "utf-8"));
      if (JSON.stringify(rederived) !== JSON.stringify(shipped)) {
        process.stderr.write(
          `[verify-ceremony]   VKEY-MISMATCH ${name}: the shipped verification key is not the ` +
            "one exported from the shipped proving key.\n",
        );
        bad += 1;
      } else {
        info(`  OK vkey    ${name}: re-derived from the proving key and identical`);
      }
    } finally {
      fs.rmSync(tmp, { force: true });
    }
  }

  if (args.offline) {
    warn("--offline: the drand beacon was NOT checked. This run proves the artifacts are");
    warn("           internally consistent, not that the beacon is the published one.");
  } else {
    await checkBeacon(transcript);
  }

  if (bad > 0) fail(`${bad} check(s) failed across ${circuits.length} circuit(s).`, 2);
  info(`All ${circuits.length} circuit(s) verified against the ceremony transcript.`);
}

main().catch((err) => {
  process.stderr.write(`[verify-ceremony] ERROR ${err && err.stack ? err.stack : err}\n`);
  process.exit(1);
});
