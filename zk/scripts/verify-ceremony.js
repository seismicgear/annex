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
const { bls12_381 } = require("@noble/curves/bls12-381.js");

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
    else if (a === "--transcript" && argv[i + 1]) out.transcript = argv[++i];
    else if (a === "--help" || a === "-h") out.help = true;
    else fail(`unknown argument: ${a}`);
  }
  return out;
}

/// drand's chained scheme signs SHA256(prev_signature || round_be64) on G2,
/// against a chain public key on G1, with this domain separation tag.
const DRAND_DST = "BLS_SIG_BLS12381G2_XMD:SHA-256_SSWU_RO_NUL_";

function hexToBytes(h) {
  if (typeof h !== "string" || h.length % 2 !== 0 || !/^[0-9a-fA-F]*$/.test(h)) {
    throw new Error(`not hex: ${String(h).slice(0, 32)}`);
  }
  return Uint8Array.from(Buffer.from(h, "hex"));
}

/**
 * Check one beacon against the drand chain it claims to come from.
 *
 * Returns true only if every check passed. The caller must honour that — see
 * the note at the call site.
 *
 * Four things are checked and the first three were previously not checked at
 * all:
 *
 *  1. The chain is the one the transcript names: `/info` must return the same
 *     chain hash AND the same public key. Comparing randomness against an
 *     endpoint chosen by the transcript proves nothing if the transcript can
 *     also choose the chain.
 *  2. The signature verifies under BLS12-381 against that chain key. This is
 *     what makes the beacon *authentic* rather than merely *agreed with by the
 *     server we asked*. Without it, anyone who could answer for api.drand.sh
 *     could mint a beacon.
 *  3. `randomness == SHA256(signature)`, which is drand's definition. The old
 *     check compared the transcript's randomness to the endpoint's and stopped
 *     there, so a transcript recording a randomness unrelated to its own
 *     signature passed.
 *  4. The commitment predates the beacon. A beacon is unpredictable
 *     randomness mixed in AFTER the contributions are fixed; if the commitment
 *     was recorded after the round was already public, the beacon adds
 *     nothing, because the operator could have chosen contributions knowing
 *     it. Round time is `genesis_time + (round - 1) * period`, which is exact,
 *     so this is checkable rather than a matter of trust.
 */
async function checkOneBeacon(label, b) {
  let ok = true;

  let chainInfo;
  try {
    const res = await fetch(`https://api.drand.sh/${b.chainHash}/info`);
    if (!res.ok) {
      warn(`${label}: drand /info returned HTTP ${res.status}; cannot authenticate the chain.`);
      return false;
    }
    chainInfo = await res.json();
  } catch (e) {
    warn(`${label}: could not reach drand (${e.message}); cannot authenticate the chain.`);
    return false;
  }

  if (chainInfo.hash !== b.chainHash) {
    fail(`${label}: drand reports chain hash ${chainInfo.hash}, transcript says ${b.chainHash}.`, 2);
  }
  if (chainInfo.public_key !== b.chainPublicKey) {
    fail(
      `${label}: chain public key mismatch.\n  transcript ${b.chainPublicKey}\n  drand      ${chainInfo.public_key}`,
      2,
    );
  }

  let round;
  try {
    const res = await fetch(`https://api.drand.sh/${b.chainHash}/public/${b.round}`);
    if (!res.ok) {
      warn(`${label}: drand round ${b.round} returned HTTP ${res.status}; could not check.`);
      return false;
    }
    round = await res.json();
  } catch (e) {
    warn(`${label}: could not fetch drand round ${b.round} (${e.message}).`);
    return false;
  }

  if (round.randomness !== b.randomness) {
    fail(
      `${label}: drand round ${b.round} randomness does not match the transcript.\n` +
        `  transcript ${b.randomness}\n  drand      ${round.randomness}\n` +
        "The ceremony's beacon is not the value the League of Entropy published.",
      2,
    );
  }
  if (round.signature !== b.signature) {
    fail(
      `${label}: drand round ${b.round} signature does not match the transcript.`,
      2,
    );
  }

  // (3) randomness is defined as SHA256 of the signature.
  const derived = crypto.createHash("sha256").update(Buffer.from(hexToBytes(b.signature))).digest("hex");
  if (derived !== b.randomness) {
    fail(
      `${label}: randomness is not SHA256(signature).\n  recorded ${b.randomness}\n  derived  ${derived}`,
      2,
    );
  }

  // (2) authenticate the signature against the chain key.
  try {
    const roundBuf = Buffer.alloc(8);
    roundBuf.writeBigUInt64BE(BigInt(b.round));
    const msg = Uint8Array.from(
      crypto
        .createHash("sha256")
        .update(Buffer.concat([Buffer.from(hexToBytes(round.previous_signature || "")), roundBuf]))
        .digest(),
    );
    const point = bls12_381.longSignatures.hash(msg, DRAND_DST);
    const valid = bls12_381.longSignatures.verify(
      hexToBytes(b.signature),
      point,
      hexToBytes(b.chainPublicKey),
    );
    if (!valid) {
      fail(
        `${label}: the beacon signature does NOT verify against the chain public key. ` +
          "The recorded beacon is not a genuine League of Entropy value.",
        2,
      );
    }
    info(`  OK beacon  ${label}: drand round ${b.round} signature verifies under BLS12-381`);
  } catch (e) {
    warn(`${label}: BLS verification could not run (${e.message}).`);
    ok = false;
  }

  // (4) the commitment must predate the beacon.
  const roundTimeSec = Number(chainInfo.genesis_time) + (Number(b.round) - 1) * Number(chainInfo.period);
  if (b.committedAt) {
    const committedSec = Date.parse(b.committedAt) / 1000;
    if (!Number.isFinite(committedSec)) {
      warn(`${label}: committedAt is not a parseable timestamp (${b.committedAt}).`);
      ok = false;
    } else if (committedSec >= roundTimeSec) {
      fail(
        `${label}: the commitment does NOT predate its beacon.\n` +
          `  committed  ${b.committedAt}\n` +
          `  round ${b.round} published ${new Date(roundTimeSec * 1000).toISOString()}\n` +
          `  committed ${(committedSec - roundTimeSec).toFixed(1)}s AFTER the beacon was public.\n` +
          "A beacon only contributes unpredictability if the contributions are fixed first. " +
          "Recorded after the fact, it proves nothing about what the operator could have known.",
        2,
      );
    } else {
      info(
        `  OK beacon  ${label}: committed ${(roundTimeSec - committedSec).toFixed(1)}s before round ${b.round} was published`,
      );
    }
  } else {
    warn(`${label}: no committedAt recorded, so the pre-beacon commitment cannot be checked.`);
    ok = false;
  }

  // The same claim, checked a second way and without reference to any clock.
  //
  // `committedAt` depends on the operator's system time, which a determined
  // operator controls. `latestRoundAtCommit` is the newest round drand had
  // published when the commitment was made, and the beacon round must be
  // strictly later than it. Neither check subsumes the other: the timestamp
  // catches a commitment made late, this catches a transcript whose clock was
  // simply wound back.
  if (b.latestRoundAtCommit !== undefined) {
    if (!(Number(b.round) > Number(b.latestRoundAtCommit))) {
      fail(
        `${label}: the beacon round ${b.round} was NOT in the future at commit time ` +
          `(drand had already published round ${b.latestRoundAtCommit}).`,
        2,
      );
    }
    info(
      `  OK beacon  ${label}: round ${b.round} was ${Number(b.round) - Number(b.latestRoundAtCommit)} round(s) in the future at commit time`,
    );
  } else {
    // Transcripts produced before `latestRoundAtCommit` was recorded still
    // verify on the timestamp alone; say so rather than passing silently.
    warn(
      `${label}: no latestRoundAtCommit recorded — the pre-beacon claim rests on the ` +
        "operator's clock alone. Re-run the ceremony to record it.",
    );
  }

  return ok;
}

async function checkBeacon(transcript) {
  const beacons = [
    transcript.phase1 && transcript.phase1.beacon && ["phase1", transcript.phase1.beacon],
    transcript.phase2 && transcript.phase2.beacon && ["phase2", transcript.phase2.beacon],
  ].filter(Boolean);

  if (beacons.length === 0) {
    warn("transcript records no beacon — nothing to check against drand.");
    return false;
  }

  let ok = true;
  for (const [label, b] of beacons) {
    if (!(await checkOneBeacon(label, b))) ok = false;
  }
  return ok;
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    process.stdout.write(
      "Usage: node zk/scripts/verify-ceremony.js [--offline] [--circuit <name>] " +
        "[--transcript <path>]\n",
    );
    process.exit(0);
  }

  // `--transcript` exists so a test can drive this against a DOCTORED
  // transcript without writing one into `zk/artifacts/ceremony/`.
  //
  // `scripts/tests/ceremony-verifier.test.sh` used to mutate the tracked file in
  // place and restore it afterwards, which is unsafe for a reason no assertion
  // inside the test can fix: between the mutation and the restore the repository
  // holds a corrupt artifact, and anything that reads the working tree in that
  // window — `git add -A`, a concurrent CI step, a person — sees it. It happened:
  // commit `7d0b2f4` shipped a transcript with a `"aaaa…"` chain public key,
  // taken from that test's own "wrong chain public key" case, and
  // `verify-ceremony.js` failed on it in CI.
  //
  // The paths the transcript names (`ptauFile.path`, and each circuit's files)
  // are resolved relative to the transcript's own directory, so a copy in /tmp
  // must sit beside the artifacts it describes — or name them absolutely. The
  // test copies the whole ceremony directory.
  const transcriptPath = args.transcript
    ? path.resolve(args.transcript)
    : path.join(CEREMONY_DIR, "transcript.json");
  const ceremonyDir = path.dirname(transcriptPath);
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
  const ptau = path.resolve(ceremonyDir, transcript.ptauFile.path);
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
  } else if (!(await checkBeacon(transcript))) {
    // Fail CLOSED.
    //
    // This was `await checkBeacon(transcript);` with the result discarded, so
    // any beacon check that ended in a warning rather than a hard `fail()` —
    // an unreachable drand, a non-200 response, a missing committedAt — left
    // `bad` at zero and the script went on to print "All N circuit(s)
    // verified against the ceremony transcript." A verifier that reports
    // success when its network check did not run is worse than no verifier,
    // because the green line is what a release reads.
    //
    // `--offline` is the supported way to say "I know the beacon is not being
    // checked", and it says so loudly in its own output.
    bad += 1;
    fail(
      "beacon verification did not complete. Re-run when drand is reachable, " +
        "or pass --offline to state explicitly that the beacon is unchecked.",
      2,
    );
  }

  if (bad > 0) fail(`${bad} check(s) failed across ${circuits.length} circuit(s).`, 2);
  info(`All ${circuits.length} circuit(s) verified against the ceremony transcript.`);
}

main().catch((err) => {
  process.stderr.write(`[verify-ceremony] ERROR ${err && err.stack ? err.stack : err}\n`);
  process.exit(1);
});
