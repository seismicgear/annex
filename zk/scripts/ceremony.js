#!/usr/bin/env node
// ceremony.js — Run the production Groth16 trusted setup and pin its output.
//
// This is the script that produces the artifacts a release ships. It is NOT
// `dev-setup-groth16.js`: that one takes `crypto.randomBytes` as its only
// entropy, keeps no record of what it did, and marks its manifests
// `ceremony.type = "dev-fixture"` so `verify-artifacts.js` refuses them under
// a production profile.
//
// WHAT THIS CEREMONY ACTUALLY PROVIDES
//
// Groth16 needs a per-circuit structured reference string whose "toxic waste"
// nobody knows. The standard construction is a multi-party computation: as
// long as ONE participant destroys their secret, the SRS is safe. This script
// runs the same construction with N local contributors plus a public
// randomness beacon as the final contribution.
//
// The beacon is what makes a small ceremony meaningful. It is a drand round
// (League of Entropy, BLS-signed, publicly verifiable forever), and the round
// is COMMITTED TO BEFORE IT EXISTS: the script publishes the round number and
// the hashes of the pre-beacon artifacts, then waits for that round to be
// produced. Nobody — including whoever runs this — can know the beacon while
// choosing their contribution, so nobody can steer the final SRS.
//
// What it does NOT provide: independent participants. A multi-party ceremony
// with contributors who do not trust each other is strictly stronger, and the
// transcript this writes is designed so such contributions can be ADDED later
// without changing anything else. `ceremony.type` records exactly what
// happened — `single-contributor-beacon` for one local contributor — and
// nothing in this pipeline will ever write `mpc` on its own.
//
// Usage:
//   node zk/scripts/ceremony.js                      # full ceremony
//   node zk/scripts/ceremony.js --contributors 3     # 3 local phase-2 rounds
//   node zk/scripts/ceremony.js --ptau <file> --ptau-sha256 <hex>
//   node zk/scripts/ceremony.js --beacon-lead 6      # commit 6 rounds ahead
//
// `--ptau` is the upgrade path: point it at a real perpetual Powers of Tau
// (e.g. `powersOfTau28_hez_final_14.ptau`) and the phase-1 half of this
// ceremony is replaced by a ceremony with hundreds of independent
// contributors. The file is hash-checked against `--ptau-sha256` and then
// verified with `snarkjs powersoftau verify`, so a substituted file cannot be
// silently wrong.
//
// Everything lands in zk/artifacts/<circuit>/ — tracked, hash-pinned, and
// read directly by production builds. zk/keys/ stays dev-only.

"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");
const { execFileSync } = require("child_process");

const ZK_DIR = path.resolve(__dirname, "..");
const BUILD_DIR = path.join(ZK_DIR, "build");
const ARTIFACTS_DIR = path.join(ZK_DIR, "artifacts");
const WORK_DIR = path.join(ZK_DIR, "ceremony-work");

// The circuits a release ships. `identity` is included even though the server
// never loads its vkey: an artifact with no manifest is an artifact no gate
// checks, and that is exactly how an unverified file reaches a bundle.
const CIRCUITS = [
  { name: "identity", version: "1.0.0", treeDepth: 0, publicSignals: ["commitment"] },
  { name: "membership", version: "1.0.0", treeDepth: 20, publicSignals: ["root", "commitment"] },
  {
    // 2.0.0: `challenge` became a constrained public input. A v2 proof is now
    // evidence of a LIVE authentication rather than a bearer credential — see
    // `crates/annex-server/src/api_zk_challenge.rs`. The wire format changed
    // (five public signals, not four), so this is a major version and the
    // previous membership_v2 artifacts do not verify proofs from this circuit
    // or vice versa.
    name: "membership_v2",
    version: "2.0.0",
    treeDepth: 20,
    publicSignals: ["root", "commitment", "nullifier", "topicHash", "challenge"],
  },
  {
    name: "channel_eligibility",
    version: "1.0.0",
    treeDepth: 20,
    publicSignals: ["root", "nullifier", "requiredRoleCode", "channelTopicHash"],
  },
  {
    name: "link_pseudonyms",
    version: "1.0.0",
    treeDepth: 0,
    publicSignals: ["nullifierA", "nullifierB", "topicHashA", "topicHashB"],
  },
  {
    name: "federation_attestation",
    version: "1.0.0",
    treeDepth: 20,
    publicSignals: ["root", "nullifier", "federationContextHash"],
  },
];

// 2^14 = 16,384 constraints. The largest circuit here is `membership_v2` at
// ~11.6k after the challenge binding landed, so the headroom is ~1.4x rather
// than the ~2.5x this comment used to claim. A circuit that grows past 16k
// needs a larger power AND a new phase 1 — the ptau is not extensible after
// `prepare phase2`, so `--ptau` cannot rescue an under-sized one.
const DEFAULT_POWER = 14;

// drand mainnet, "default" chain: 30s rounds, BLS-signed, chained scheme.
// Recorded in the transcript so a verifier knows which chain to check against.
const DRAND = {
  api: "https://api.drand.sh",
  chainHash: "8990e7a9aaed2ffed73dbd7092123d6f289930540d7651336225dc172e51b2ce",
  periodSeconds: 30,
  publicKey:
    "868f005eb8e6e4ca0a47c8a77ceaa5309a47978a7c71bc5cce96366b5d7a569937c529eeda66c7293784a9402801af31",
};

function info(msg) {
  process.stdout.write(`[ceremony] ${msg}\n`);
}

function fail(msg, code = 1) {
  process.stderr.write(`[ceremony] ERROR ${msg}\n`);
  process.exit(code);
}

function sha256(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

/**
 * Run snarkjs and return its stdout.
 *
 * `execFileSync` rather than a shell string: several arguments here are file
 * paths and one is operator-supplied (`--ptau`), and a path containing a space
 * or a quote must not become two arguments or, worse, a second command.
 *
 * Output is captured rather than inherited because the contribution hashes —
 * the only durable evidence of what each round actually did — are printed to
 * stdout and nowhere else. It is echoed so a long run is still watchable.
 */
function snarkjs(args, { quiet = false } = {}) {
  const out = execFileSync(
    process.execPath,
    [path.join(ZK_DIR, "node_modules", "snarkjs", "build", "cli.cjs"), ...args],
    { cwd: ZK_DIR, encoding: "utf-8", maxBuffer: 64 * 1024 * 1024 },
  );
  if (!quiet) process.stdout.write(out);
  return out;
}

/**
 * The contribution hash snarkjs printed, as one line of hex.
 *
 * snarkjs prints it as a labelled block of four 16-byte groups across four
 * lines. Flattening it here keeps the transcript diffable and lets a verifier
 * compare it against their own `zkey verify` output without reformatting.
 */
function parseContributionHash(output) {
  const idx = output.indexOf("Contribution Hash:");
  if (idx === -1) return null;
  const tail = output.slice(idx + "Contribution Hash:".length);
  const hex = (tail.match(/[0-9a-f]{2}(\s+[0-9a-f]{2})*/gi) || [])
    .join(" ")
    .replace(/\s+/g, "");
  return hex.slice(0, 128) || null;
}

async function drandGet(pathname) {
  const res = await fetch(`${DRAND.api}${pathname}`);
  if (!res.ok) throw new Error(`drand ${pathname} returned HTTP ${res.status}`);
  return res.json();
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

/**
 * Commit to a future drand round, wait for it, and return its randomness.
 *
 * The commitment is the point. `commitment` is written to the transcript
 * BEFORE the round exists, naming the round number and the hashes of every
 * artifact that will be fed into the beacon. Anyone can later fetch that round
 * from any drand node, check the BLS signature against the chain public key,
 * and confirm the value used here is the value the League of Entropy produced
 * — which nobody could have known when the contributions were made.
 */
async function beaconFromFutureRound(leadRounds, commitment) {
  const latest = await drandGet("/public/latest");
  const targetRound = latest.round + leadRounds;

  // Stamp the commitment NOW, before the wait — not when the round arrives.
  //
  // This used to be `committedAt: new Date().toISOString()` inside the return
  // below, which runs AFTER the loop has waited for the round to be published.
  // The protocol was right — the artifact hashes are fixed here, and the round
  // committed to does not yet exist — but the recorded timestamp described the
  // moment of collection rather than the moment of commitment, so it always
  // landed at or after the beacon's own publication time.
  //
  // That made a sound ceremony indistinguishable from an unsound one. The
  // phase-2 record of the previous ceremony read as "committed 2.4s AFTER the
  // beacon was public", which is exactly what choosing contributions with
  // knowledge of the beacon would look like. `verify-ceremony.js` now checks
  // this and fails on it, so the field has to mean what it says.
  //
  // `latestRoundAtCommit` is recorded alongside so the claim is checkable a
  // second way, independent of any clock: the target round must be strictly
  // greater than the newest round that existed when the commitment was made.
  const committedAt = new Date().toISOString();

  info(
    `committing to drand round ${targetRound} (latest is ${latest.round}, ` +
      `~${leadRounds * DRAND.periodSeconds}s away)`,
  );
  info(`commitment: ${JSON.stringify(commitment)}`);

  for (;;) {
    try {
      const r = await drandGet(`/public/${targetRound}`);
      info(`drand round ${targetRound} randomness ${r.randomness}`);
      return {
        chainHash: DRAND.chainHash,
        chainPublicKey: DRAND.publicKey,
        round: targetRound,
        randomness: r.randomness,
        signature: r.signature,
        committedAt,
        latestRoundAtCommit: latest.round,
        commitment,
      };
    } catch {
      await sleep(5000);
    }
  }
}

function parseArgs(argv) {
  const out = { contributors: 1, power: DEFAULT_POWER, beaconLead: 4, beaconIters: 10 };
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    const next = () => {
      if (argv[i + 1] === undefined) fail(`${a} requires a value`);
      return argv[++i];
    };
    if (a === "--contributors") out.contributors = Number(next());
    else if (a === "--power") out.power = Number(next());
    else if (a === "--beacon-lead") out.beaconLead = Number(next());
    else if (a === "--beacon-iters") out.beaconIters = Number(next());
    else if (a === "--ptau") out.ptau = path.resolve(next());
    else if (a === "--ptau-sha256") out.ptauSha256 = next().toLowerCase();
    else if (a === "--help" || a === "-h") out.help = true;
    else fail(`unknown argument: ${a}`);
  }
  if (!Number.isInteger(out.contributors) || out.contributors < 1) {
    fail("--contributors must be a positive integer");
  }
  if (!Number.isInteger(out.power) || out.power < 8 || out.power > 24) {
    fail("--power must be an integer in 8..=24");
  }
  if (!Number.isInteger(out.beaconLead) || out.beaconLead < 1) {
    fail("--beacon-lead must be a positive integer (rounds to wait)");
  }
  return out;
}

function printHelp() {
  process.stdout.write(
    [
      "Usage: node zk/scripts/ceremony.js [options]",
      "",
      "  --contributors <n>    local phase-2 contribution rounds (default 1)",
      "  --power <n>           Powers of Tau size, 2^n constraints (default 14)",
      "  --beacon-lead <n>     drand rounds to commit ahead (default 4, 30s each)",
      "  --beacon-iters <n>    beacon PRF iterations exponent (default 10)",
      "  --ptau <file>         use an existing phase-1 ptau instead of generating one",
      "  --ptau-sha256 <hex>   required hash for --ptau; refuses a mismatch",
      "",
      "Writes zk/artifacts/<circuit>/{wasm,zkey,vkey,r1cs,manifest.json}",
      "and zk/artifacts/ceremony/transcript.json.",
      "",
    ].join("\n"),
  );
}

/**
 * Phase 1 — the Powers of Tau, shared by every circuit.
 *
 * Either adopts an operator-supplied ptau (hash-checked, then verified) or
 * generates one: new → local contribution → drand beacon → prepare phase2.
 * Both paths end with `powersoftau verify`, which checks the whole
 * contribution chain rather than just the file's shape.
 */
async function phase1(args, transcript) {
  const finalPtau = path.join(WORK_DIR, `pot${args.power}_final.ptau`);

  if (args.ptau) {
    if (!fs.existsSync(args.ptau)) fail(`--ptau file not found: ${args.ptau}`);
    const actual = sha256(args.ptau);
    if (!args.ptauSha256) {
      fail(
        "--ptau requires --ptau-sha256. An unpinned phase-1 file is an unverifiable " +
          `input to every proving key in this repo. Measured sha256: ${actual}`,
      );
    }
    if (actual !== args.ptauSha256) {
      fail(
        `--ptau hash mismatch\n  expected ${args.ptauSha256}\n  actual   ${actual}`,
        2,
      );
    }
    info(`adopting operator-supplied ptau (sha256 ${actual.slice(0, 16)}…)`);
    fs.copyFileSync(args.ptau, finalPtau);
    info("verifying the supplied ptau's contribution chain...");
    snarkjs(["powersoftau", "verify", finalPtau]);
    transcript.phase1 = {
      source: "operator-supplied",
      originalPath: args.ptau,
      sha256: actual,
      power: args.power,
      verified: true,
    };
    return finalPtau;
  }

  const ptau0 = path.join(WORK_DIR, `pot${args.power}_0000.ptau`);
  const ptau1 = path.join(WORK_DIR, `pot${args.power}_0001.ptau`);
  const ptauBeacon = path.join(WORK_DIR, `pot${args.power}_beacon.ptau`);

  info(`phase 1: new Powers of Tau, 2^${args.power} constraints`);
  snarkjs(["powersoftau", "new", "bn128", String(args.power), ptau0, "-v"], { quiet: true });

  info("phase 1: local contribution");
  const entropy = crypto.randomBytes(32).toString("hex");
  const contribOut = snarkjs(
    ["powersoftau", "contribute", ptau0, ptau1, "--name=annex-phase1-local-1", "-v", `-e=${entropy}`],
    { quiet: true },
  );
  const contribHash = parseContributionHash(contribOut);
  info(`phase 1: contribution hash ${contribHash ? contribHash.slice(0, 32) + "…" : "(unreported)"}`);

  const beacon = await beaconFromFutureRound(args.beaconLead, {
    stage: "phase1",
    preBeaconPtauSha256: sha256(ptau1),
  });

  info("phase 1: applying the drand beacon as the final contribution");
  snarkjs(
    ["powersoftau", "beacon", ptau1, ptauBeacon, beacon.randomness, String(args.beaconIters), "-n=drand-beacon"],
    { quiet: true },
  );

  info("phase 1: prepare phase2");
  snarkjs(["powersoftau", "prepare", "phase2", ptauBeacon, finalPtau, "-v"], { quiet: true });

  info("phase 1: verifying the contribution chain");
  snarkjs(["powersoftau", "verify", finalPtau]);

  transcript.phase1 = {
    source: "generated-by-this-ceremony",
    power: args.power,
    contributions: [{ name: "annex-phase1-local-1", hash: contribHash }],
    beacon,
    preparedSha256: sha256(finalPtau),
    verified: true,
  };
  return finalPtau;
}

/**
 * Phase 2 — one proving key per circuit, from the shared ptau.
 *
 * Contributions first, then a single beacon committed to after they are all
 * made, so one beacon covers every circuit and a verifier has one round to
 * check rather than six.
 */
async function phase2(args, ptau, transcript) {
  const staged = [];

  for (const circuit of CIRCUITS) {
    const r1cs = path.join(BUILD_DIR, `${circuit.name}.r1cs`);
    if (!fs.existsSync(r1cs)) {
      fail(`${r1cs} is missing. Run \`node zk/scripts/build-circuits.js\` first.`, 2);
    }
    info(`phase 2: ${circuit.name} — groth16 setup`);
    let current = path.join(WORK_DIR, `${circuit.name}_0000.zkey`);
    snarkjs(["groth16", "setup", r1cs, ptau, current], { quiet: true });

    const contributions = [];
    for (let i = 1; i <= args.contributors; i++) {
      const next = path.join(WORK_DIR, `${circuit.name}_${String(i).padStart(4, "0")}.zkey`);
      const name = `annex-phase2-local-${i}`;
      const out = snarkjs(
        ["zkey", "contribute", current, next, `--name=${name}`, "-v", `-e=${crypto.randomBytes(32).toString("hex")}`],
        { quiet: true },
      );
      contributions.push({ name, hash: parseContributionHash(out) });
      current = next;
      info(`phase 2: ${circuit.name} — contribution ${i}/${args.contributors}`);
    }
    staged.push({ circuit, r1cs, preBeacon: current, contributions });
  }

  const beacon = await beaconFromFutureRound(args.beaconLead, {
    stage: "phase2",
    preBeaconZkeySha256: Object.fromEntries(
      staged.map((s) => [s.circuit.name, sha256(s.preBeacon)]),
    ),
  });

  const results = [];
  for (const s of staged) {
    const name = s.circuit.name;
    const finalZkey = path.join(WORK_DIR, `${name}_final.zkey`);
    const vkey = path.join(WORK_DIR, `${name}_vkey.json`);

    info(`phase 2: ${name} — applying the drand beacon`);
    snarkjs(
      ["zkey", "beacon", s.preBeacon, finalZkey, beacon.randomness, String(args.beaconIters), "-n=drand-beacon"],
      { quiet: true },
    );

    // The load-bearing check. `zkey verify` walks the entire chain — r1cs to
    // ptau to every contribution to the beacon — and fails if any link is
    // wrong. Without it the manifest would only prove the files have not
    // changed SINCE the ceremony, not that the ceremony produced them.
    info(`phase 2: ${name} — snarkjs zkey verify`);
    snarkjs(["zkey", "verify", s.r1cs, ptau, finalZkey]);

    snarkjs(["zkey", "export", "verificationkey", finalZkey, vkey], { quiet: true });
    results.push({ ...s, finalZkey, vkey, beacon });
  }

  transcript.phase2 = {
    contributorsPerCircuit: args.contributors,
    beacon,
    circuits: results.map((r) => ({
      circuit: r.circuit.name,
      contributions: r.contributions,
      verified: true,
    })),
  };
  return results;
}

/** Copy the ceremony's output into the tracked artifact tree and pin it. */
function publish(results, ptau, transcript, args) {
  fs.mkdirSync(path.join(ARTIFACTS_DIR, "ceremony"), { recursive: true });
  const ptauDest = path.join(ARTIFACTS_DIR, "ceremony", path.basename(ptau));
  fs.copyFileSync(ptau, ptauDest);
  transcript.ptauFile = { path: `./${path.basename(ptau)}`, sha256: sha256(ptauDest) };

  for (const r of results) {
    const name = r.circuit.name;
    const dir = path.join(ARTIFACTS_DIR, name);
    fs.mkdirSync(dir, { recursive: true });

    const wasmSrc = path.join(BUILD_DIR, `${name}_js`, `${name}.wasm`);
    if (!fs.existsSync(wasmSrc)) fail(`${wasmSrc} is missing — rebuild the circuits.`, 2);

    const files = {
      wasm: path.join(dir, `${name}.wasm`),
      zkey: path.join(dir, `${name}_final.zkey`),
      vkey: path.join(dir, `${name}_vkey.json`),
      r1cs: path.join(dir, `${name}.r1cs`),
    };
    fs.copyFileSync(wasmSrc, files.wasm);
    fs.copyFileSync(r.finalZkey, files.zkey);
    fs.copyFileSync(r.vkey, files.vkey);
    fs.copyFileSync(r.r1cs, files.r1cs);

    const manifest = {
      schemaVersion: 1,
      circuit: name,
      circuitVersion: r.circuit.version,
      curve: "bn254",
      provingSystem: "groth16",
      treeDepth: r.circuit.treeDepth,
      publicSignals: r.circuit.publicSignals,
      wasm_sha256: sha256(files.wasm),
      zkey_sha256: sha256(files.zkey),
      vkey_sha256: sha256(files.vkey),
      r1cs_sha256: sha256(files.r1cs),
      paths: {
        wasm: `./${name}.wasm`,
        zkey: `./${name}_final.zkey`,
        vkey: `./${name}_vkey.json`,
        r1cs: `./${name}.r1cs`,
      },
      ceremony: {
        // Never "mpc". This value is derived from what actually happened:
        // one local contributor per circuit plus a committed public beacon.
        // Add independent contributors and it becomes "multi-contributor-beacon".
        type: args.contributors > 1 ? "multi-contributor-beacon" : "single-contributor-beacon",
        generatedAt: transcript.startedAt,
        transcript: "../ceremony/transcript.json",
        phase1Source: transcript.phase1.source,
        beaconRound: transcript.phase2.beacon.round,
        beaconChainHash: transcript.phase2.beacon.chainHash,
        zkeyVerified: true,
        note:
          "Produced by zk/scripts/ceremony.js. Phase 2 was finalised with drand round " +
          `${transcript.phase2.beacon.round} on chain ${transcript.phase2.beacon.chainHash}, ` +
          "committed to before that round existed (see the transcript's commitment field). " +
          "`snarkjs zkey verify` passed against the pinned r1cs and ptau. This is NOT a " +
          "multi-party ceremony with independent participants; see " +
          "docs/refactor/zk-merkle-production.md for what that would add.",
      },
    };
    fs.writeFileSync(path.join(dir, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
    info(`published zk/artifacts/${name}/ (zkey ${manifest.zkey_sha256.slice(0, 16)}…)`);
  }

  transcript.finishedAt = new Date().toISOString();
  fs.writeFileSync(
    path.join(ARTIFACTS_DIR, "ceremony", "transcript.json"),
    JSON.stringify(transcript, null, 2) + "\n",
  );
  info("published zk/artifacts/ceremony/transcript.json");
}

async function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    printHelp();
    process.exit(0);
  }

  const transcript = {
    schemaVersion: 1,
    startedAt: new Date().toISOString(),
    tool: "zk/scripts/ceremony.js",
    snarkjs: require(path.join(ZK_DIR, "node_modules", "snarkjs", "package.json")).version,
    circom: "v2.2.3",
    drandChain: DRAND,
  };

  fs.rmSync(WORK_DIR, { recursive: true, force: true });
  fs.mkdirSync(WORK_DIR, { recursive: true });

  const ptau = await phase1(args, transcript);
  const results = await phase2(args, ptau, transcript);
  publish(results, ptau, transcript, args);

  info("ceremony complete.");
  info("Verify it from a clean checkout with:");
  info("  node zk/scripts/verify-artifacts.js --all --profile production");
  info("  node zk/scripts/verify-ceremony.js");
}

main().catch((err) => {
  process.stderr.write(`[ceremony] ERROR ${err && err.stack ? err.stack : err}\n`);
  process.exit(1);
});
