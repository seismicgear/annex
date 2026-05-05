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
//
// Exit codes:
//   0  every required artifact exists and matches the manifest hash
//   1  manifest missing / unparseable / unsupported schema
//   2  one or more artifacts missing or hash-mismatched
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
    "Usage: node zk/scripts/verify-artifacts.js [--manifest <path>]\n"
  );
  process.stdout.write(
    "\nDefault manifest: zk/artifacts/membership/manifest.json\n"
  );
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
  info(`circuit:         ${manifest.circuit} (version ${manifest.circuitVersion})`);
  info(
    `proving system:  ${manifest.provingSystem} over ${manifest.curve}, tree depth ${manifest.treeDepth}`
  );
  info(`public signals:  [${manifest.publicSignals.join(", ")}]`);

  if (manifest.ceremony && manifest.ceremony.type === "dev-fixture") {
    warn(
      "manifest is marked ceremony.type=\"dev-fixture\". The pinned hashes refer to artifacts produced by random-entropy dev setup — NOT a real ceremony. A public production release MUST replace these with real ceremony output before tagging."
    );
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
