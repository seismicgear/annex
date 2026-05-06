#!/usr/bin/env node
// verify-artifacts.js — Verify ZK artifacts against a pinned manifest.
//
// Reads zk/artifacts/<circuit>/manifest.json (default: membership), resolves
// every path inside `paths` relative to the manifest's directory, computes
// its SHA-256, and fails non-zero if any artifact is missing or mismatched.
//
// Usage:
//   node zk/scripts/verify-artifacts.js
//   node zk/scripts/verify-artifacts.js --manifest <path/to/manifest.json>
//   node zk/scripts/verify-artifacts.js --profile production
//
// Profile is taken from --profile, otherwise from the ANNEX_BUILD_PROFILE
// environment variable (lower-cased). Recognised values: "dev" (default),
// "production" / "release". Under a production profile, a manifest marked
// `ceremony.type = "dev-fixture"` is REJECTED unless the operator opts in
// with `ANNEX_ALLOW_DEV_CEREMONY=1` (e.g. for staging dry-runs). This is
// the gate that prevents a release build from silently shipping random-
// entropy dev keys.
//
// Exit codes:
//   0  every required artifact exists and matches the manifest hash
//   1  manifest missing / unparseable / unsupported schema
//   2  one or more artifacts missing or hash-mismatched
//   3  production profile but manifest is dev-fixture (and not opted in)
//
// This script is intentionally side-effect-free: it never writes, downloads,
// or regenerates anything. Production builds should call it before consuming
// ZK artifacts.

"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const REQUIRED_FIELDS = [
  "schemaVersion",
  "circuit",
  "circuitVersion",
  "curve",
  "provingSystem",
  "treeDepth",
  "publicSignals",
  "wasm_sha256",
  "zkey_sha256",
  "vkey_sha256",
  "paths",
];

const REQUIRED_PATHS = ["wasm", "zkey", "vkey"];

function info(msg) {
  process.stdout.write(`[verify-artifacts] ${msg}\n`);
}

function warn(msg) {
  process.stdout.write(`[verify-artifacts] WARN ${msg}\n`);
}

function fail(msg, code = 1) {
  process.stderr.write(`[verify-artifacts] ERROR ${msg}\n`);
  process.exit(code);
}

function sha256OfFile(filePath) {
  const hash = crypto.createHash("sha256");
  const buf = fs.readFileSync(filePath);
  hash.update(buf);
  return hash.digest("hex");
}

function parseArgs(argv) {
  const out = {};
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--manifest" && argv[i + 1]) {
      out.manifest = argv[++i];
    } else if (a === "--profile" && argv[i + 1]) {
      out.profile = argv[++i];
    } else if (a === "--help" || a === "-h") {
      out.help = true;
    } else {
      fail(`unknown argument: ${a}`);
    }
  }
  return out;
}

function printHelp() {
  process.stdout.write(
    "Usage: node zk/scripts/verify-artifacts.js [--manifest <path>] [--profile <dev|production>]\n"
  );
  process.stdout.write(
    "\nDefault manifest: zk/artifacts/membership/manifest.json\n" +
      "Default profile:  $ANNEX_BUILD_PROFILE (or \"dev\")\n" +
      "\nUnder --profile production, a manifest with ceremony.type=\"dev-fixture\"\n" +
      "is rejected unless ANNEX_ALLOW_DEV_CEREMONY=1.\n"
  );
}

/// Normalise a profile string, returning "production" / "dev" / null. Anything
/// other than the recognised aliases falls through to null so the caller can
/// surface a clear error.
function normaliseProfile(raw) {
  if (raw === undefined || raw === null) return null;
  const v = String(raw).trim().toLowerCase();
  if (v === "") return null;
  if (v === "production" || v === "release") return "production";
  if (v === "dev" || v === "development") return "dev";
  return v; // unknown — caller decides
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  if (args.help) {
    printHelp();
    process.exit(0);
  }

  const manifestPath = args.manifest
    ? path.resolve(args.manifest)
    : path.resolve(__dirname, "..", "artifacts", "membership", "manifest.json");

  // Resolve the build profile. CLI flag wins; otherwise fall back to the
  // env var. An unrecognised value is a hard error: silently treating
  // `ANNEX_BUILD_PROFILE=produktion` as dev would defeat the whole point
  // of the gate.
  const profileRaw = args.profile !== undefined
    ? args.profile
    : process.env.ANNEX_BUILD_PROFILE;
  const profile = normaliseProfile(profileRaw) ?? "dev";
  if (profile !== "production" && profile !== "dev") {
    fail(
      `unrecognised build profile "${profileRaw}" — use "dev" or "production".`
    );
  }
  const isProduction = profile === "production";
  const allowDevCeremony = process.env.ANNEX_ALLOW_DEV_CEREMONY === "1";

  if (!fs.existsSync(manifestPath)) {
    fail(
      `manifest not found at ${manifestPath}. ` +
        `Production builds require a pinned manifest; see docs/refactor/zk-merkle-production.md.`
    );
  }

  let manifest;
  try {
    manifest = JSON.parse(fs.readFileSync(manifestPath, "utf-8"));
  } catch (e) {
    fail(`manifest at ${manifestPath} is not valid JSON: ${e.message}`);
  }

  for (const f of REQUIRED_FIELDS) {
    if (manifest[f] === undefined) {
      fail(`manifest is missing required field: ${f}`);
    }
  }

  if (manifest.schemaVersion !== 1) {
    fail(
      `unsupported manifest schemaVersion=${manifest.schemaVersion} (expected 1)`
    );
  }

  for (const p of REQUIRED_PATHS) {
    if (typeof manifest.paths[p] !== "string") {
      fail(`manifest.paths.${p} must be a string`);
    }
  }

  info(`manifest:        ${manifestPath}`);
  info(`profile:         ${profile}`);
  info(`circuit:         ${manifest.circuit} (version ${manifest.circuitVersion})`);
  info(
    `proving system:  ${manifest.provingSystem} over ${manifest.curve}, tree depth ${manifest.treeDepth}`
  );
  info(`public signals:  [${manifest.publicSignals.join(", ")}]`);

  // Ceremony gate. Under a production profile, a `dev-fixture` ceremony is a
  // hard fail unless the operator explicitly opts in via
  // `ANNEX_ALLOW_DEV_CEREMONY=1`. The opt-in exists so a staging release can
  // still be cut while the real ceremony is being scheduled, without losing
  // the production gate for tag-driven releases.
  if (manifest.ceremony && manifest.ceremony.type === "dev-fixture") {
    if (isProduction && !allowDevCeremony) {
      fail(
        `manifest is marked ceremony.type="dev-fixture" but ANNEX_BUILD_PROFILE=${profile}. ` +
          `Refusing to verify dev-fixture artifacts under a production profile. ` +
          `Replace the manifest + artifacts with multi-party ceremony output, ` +
          `or set ANNEX_ALLOW_DEV_CEREMONY=1 to opt in (e.g. for staging dry-runs).`,
        3
      );
    }
    warn(
      "manifest is marked ceremony.type=\"dev-fixture\". The pinned hashes refer to artifacts produced by random-entropy dev setup — NOT a real ceremony. A public production release MUST replace these with real ceremony output before tagging."
    );
    if (isProduction && allowDevCeremony) {
      warn(
        "ANNEX_ALLOW_DEV_CEREMONY=1 — proceeding with dev-fixture artifacts under a production profile. This must NEVER be used for a public release."
      );
    }
  }

  const manifestDir = path.dirname(manifestPath);

  const checks = [
    { name: "wasm", expected: manifest.wasm_sha256 },
    { name: "zkey", expected: manifest.zkey_sha256 },
    { name: "vkey", expected: manifest.vkey_sha256 },
  ];
  if (manifest.r1cs_sha256 && manifest.paths.r1cs) {
    checks.push({ name: "r1cs", expected: manifest.r1cs_sha256 });
  }

  let bad = 0;
  for (const c of checks) {
    const rel = manifest.paths[c.name];
    if (!rel) {
      process.stderr.write(
        `[verify-artifacts]   MISSING-PATH ${c.name}: manifest.paths.${c.name} not set\n`
      );
      bad += 1;
      continue;
    }
    const filePath = path.resolve(manifestDir, rel);
    if (!fs.existsSync(filePath)) {
      process.stderr.write(
        `[verify-artifacts]   MISSING-FILE ${c.name}: ${filePath}\n`
      );
      bad += 1;
      continue;
    }
    const actual = sha256OfFile(filePath);
    if (typeof c.expected !== "string" || c.expected.length !== 64) {
      process.stderr.write(
        `[verify-artifacts]   BAD-MANIFEST  ${c.name}: pinned sha256 must be a 64-char hex string\n`
      );
      bad += 1;
      continue;
    }
    if (actual.toLowerCase() !== c.expected.toLowerCase()) {
      process.stderr.write(
        `[verify-artifacts]   MISMATCH      ${c.name}: ${filePath}\n`
      );
      process.stderr.write(`[verify-artifacts]     expected ${c.expected}\n`);
      process.stderr.write(`[verify-artifacts]     actual   ${actual}\n`);
      bad += 1;
      continue;
    }
    info(`  OK ${c.name.padEnd(4)} ${filePath} (sha256 ${actual.slice(0, 16)}…)`);
  }

  if (bad > 0) {
    fail(`${bad} artifact(s) missing or hash-mismatched. Refusing to proceed.`, 2);
  }

  info("All artifacts verified against manifest.");
}

main();
