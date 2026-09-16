#!/usr/bin/env node
// install-ceremony.js — Put the pinned ceremony artifacts where the build
// expects to find them.
//
// The tracked, hash-pinned artifacts live in `zk/artifacts/<circuit>/` beside
// the manifest that pins them. Everything downstream — `scripts/build-desktop.js`,
// `scripts/prepare-zk-dev.js`, `crates/annex-desktop/tauri.conf.json`'s
// `bundle.resources`, `scripts/smoke-server.sh`, the server's own vkey loader —
// looks in `zk/keys/` and `zk/build/`. Those two directories are the DEV
// working area: `dev-setup-groth16.js` fills them with random-entropy keys and
// `.gitignore` excludes them.
//
// Rather than teach six consumers about a second location, this copies. That
// is deliberate: the fewer things a release path touches, the fewer lanes it
// can break, and every one of those consumers is exercised by a CI job that
// would have to be re-proved.
//
// Copying is only safe if the copy is checked, so each file is re-hashed after
// it lands and compared against the manifest. A truncated copy or a full disk
// would otherwise produce a bundle whose vkey is not the vkey that was
// verified thirty seconds earlier — precisely the "value that never crosses a
// boundary it is assumed to cross" defect this codebase produces.
//
// Usage:
//   node zk/scripts/install-ceremony.js            # all circuits
//   node zk/scripts/install-ceremony.js --circuit membership
//
// Exit codes:
//   0  installed and re-verified
//   1  usage / missing ceremony artifacts
//   2  a copy did not match its manifest hash

"use strict";

const fs = require("fs");
const path = require("path");
const crypto = require("crypto");

const ZK_DIR = path.resolve(__dirname, "..");
const ARTIFACTS_DIR = path.join(ZK_DIR, "artifacts");
const KEYS_DIR = path.join(ZK_DIR, "keys");
const BUILD_DIR = path.join(ZK_DIR, "build");

function info(msg) {
  process.stdout.write(`[install-ceremony] ${msg}\n`);
}
function fail(msg, code = 1) {
  process.stderr.write(`[install-ceremony] ERROR ${msg}\n`);
  process.exit(code);
}
function sha256(file) {
  return crypto.createHash("sha256").update(fs.readFileSync(file)).digest("hex");
}

function copyChecked(src, dest, expected, label) {
  fs.mkdirSync(path.dirname(dest), { recursive: true });
  fs.copyFileSync(src, dest);
  const actual = sha256(dest);
  if (expected && actual !== expected) {
    fail(
      `${label}: the installed copy does not match the manifest.\n` +
        `  source   ${src}\n  dest     ${dest}\n` +
        `  expected ${expected}\n  actual   ${actual}`,
      2,
    );
  }
  return actual;
}

function main() {
  const args = process.argv.slice(2);
  let only = null;
  for (let i = 0; i < args.length; i++) {
    if (args[i] === "--circuit" && args[i + 1]) only = args[++i];
    else if (args[i] === "--help" || args[i] === "-h") {
      process.stdout.write("Usage: node zk/scripts/install-ceremony.js [--circuit <name>]\n");
      process.exit(0);
    } else fail(`unknown argument: ${args[i]}`);
  }

  if (!fs.existsSync(path.join(ARTIFACTS_DIR, "ceremony", "transcript.json"))) {
    fail(
      "no ceremony transcript at zk/artifacts/ceremony/transcript.json. There is nothing to " +
        "install: run `node zk/scripts/ceremony.js`, or use the dev path " +
        "(`node zk/scripts/dev-setup-groth16.js`) for local work.",
    );
  }

  const circuits = fs
    .readdirSync(ARTIFACTS_DIR, { withFileTypes: true })
    .filter((d) => d.isDirectory() && d.name !== "ceremony")
    .map((d) => d.name)
    .filter((n) => !only || n === only)
    .sort();
  if (circuits.length === 0) fail(only ? `no artifacts for circuit ${only}` : "no circuits found");

  for (const name of circuits) {
    const dir = path.join(ARTIFACTS_DIR, name);
    const manifestPath = path.join(dir, "manifest.json");
    if (!fs.existsSync(manifestPath)) fail(`${name} has no manifest.json`);
    const m = JSON.parse(fs.readFileSync(manifestPath, "utf-8"));

    const src = (rel) => path.resolve(dir, rel);

    copyChecked(
      src(m.paths.zkey),
      path.join(KEYS_DIR, `${name}_final.zkey`),
      m.zkey_sha256,
      `${name} zkey`,
    );
    copyChecked(
      src(m.paths.vkey),
      path.join(KEYS_DIR, `${name}_vkey.json`),
      m.vkey_sha256,
      `${name} vkey`,
    );
    // The wasm goes where circom would have put it, so `prepare-zk-dev.js` and
    // `build-desktop.js` find it at the path they already know.
    copyChecked(
      src(m.paths.wasm),
      path.join(BUILD_DIR, `${name}_js`, `${name}.wasm`),
      m.wasm_sha256,
      `${name} wasm`,
    );
    if (m.paths.r1cs) {
      copyChecked(
        src(m.paths.r1cs),
        path.join(BUILD_DIR, `${name}.r1cs`),
        m.r1cs_sha256,
        `${name} r1cs`,
      );
    }
    info(`installed ${name}`);
  }

  info(`Installed ${circuits.length} circuit(s) into zk/keys and zk/build, hashes re-checked.`);
}

main();
