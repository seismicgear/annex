#!/usr/bin/env node
// verify-artifacts.test.js — unit tests for the dev-fixture gate.
//
// These run under node:test (built into Node 18+). They write a temporary
// manifest + a few stub artifact files into a fresh tmpdir, then spawn
// `verify-artifacts.js --manifest <tmp>/manifest.json` with various profile
// + env settings and assert exit codes + stderr substrings.
//
// Running:
//   node --test zk/scripts/verify-artifacts.test.js

"use strict";

const test = require("node:test");
const assert = require("node:assert");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const crypto = require("node:crypto");
const { spawnSync } = require("node:child_process");

const SCRIPT = path.resolve(__dirname, "verify-artifacts.js");

function makeStubArtifacts(dir, withR1cs) {
  // Build a minimal "valid" tree: artifacts of fixed bytes and a manifest
  // that pins the matching SHA-256.
  fs.mkdirSync(dir, { recursive: true });
  const wasmBuf = Buffer.from("wasm-bytes");
  const zkeyBuf = Buffer.from("zkey-bytes");
  const vkeyBuf = Buffer.from("vkey-bytes");
  const r1csBuf = Buffer.from("r1cs-bytes");
  fs.writeFileSync(path.join(dir, "membership.wasm"), wasmBuf);
  fs.writeFileSync(path.join(dir, "membership.zkey"), zkeyBuf);
  fs.writeFileSync(path.join(dir, "membership.vkey.json"), vkeyBuf);
  if (withR1cs) {
    fs.writeFileSync(path.join(dir, "membership.r1cs"), r1csBuf);
  }
  const sha256 = (b) => crypto.createHash("sha256").update(b).digest("hex");
  const manifest = {
    schemaVersion: 1,
    circuit: "membership",
    circuitVersion: "test-1",
    curve: "bn254",
    provingSystem: "groth16",
    treeDepth: 20,
    publicSignals: ["root", "commitment"],
    wasm_sha256: sha256(wasmBuf),
    zkey_sha256: sha256(zkeyBuf),
    vkey_sha256: sha256(vkeyBuf),
    paths: {
      wasm: "membership.wasm",
      zkey: "membership.zkey",
      vkey: "membership.vkey.json",
    },
  };
  if (withR1cs) {
    manifest.r1cs_sha256 = sha256(r1csBuf);
    manifest.paths.r1cs = "membership.r1cs";
  }
  return manifest;
}

function writeManifest(dir, manifest) {
  const p = path.join(dir, "manifest.json");
  fs.writeFileSync(p, JSON.stringify(manifest, null, 2));
  return p;
}

function tmpDir() {
  return fs.mkdtempSync(path.join(os.tmpdir(), "verify-artifacts-test-"));
}

function runScript(args, env) {
  const merged = { ...process.env };
  // Strip every ANNEX_ env var so tests are hermetic, then layer on what
  // the test wants. Without this, an outer `ANNEX_BUILD_PROFILE=...` would
  // leak in.
  for (const k of Object.keys(merged)) {
    if (k.startsWith("ANNEX_")) delete merged[k];
  }
  Object.assign(merged, env || {});
  return spawnSync("node", [SCRIPT, ...args], {
    encoding: "utf-8",
    env: merged,
  });
}

test("dev profile: ceremony omitted → success", () => {
  const dir = tmpDir();
  const manifest = makeStubArtifacts(dir, true);
  // No `ceremony` block at all.
  const manifestPath = writeManifest(dir, manifest);
  const r = runScript(["--manifest", manifestPath]);
  assert.strictEqual(r.status, 0, `stderr: ${r.stderr}\nstdout: ${r.stdout}`);
  assert.match(r.stdout, /All artifacts verified against manifest/);
});

test("dev profile: ceremony=dev-fixture → success with warning", () => {
  const dir = tmpDir();
  const manifest = makeStubArtifacts(dir, false);
  manifest.ceremony = { type: "dev-fixture", note: "test fixture" };
  const manifestPath = writeManifest(dir, manifest);
  const r = runScript(["--manifest", manifestPath]);
  assert.strictEqual(r.status, 0, `stderr: ${r.stderr}\nstdout: ${r.stdout}`);
  assert.match(r.stdout, /WARN.*dev-fixture/);
});

test("production profile: ceremony=dev-fixture → exit 3", () => {
  const dir = tmpDir();
  const manifest = makeStubArtifacts(dir, false);
  manifest.ceremony = { type: "dev-fixture" };
  const manifestPath = writeManifest(dir, manifest);
  const r = runScript(["--manifest", manifestPath, "--profile", "production"]);
  assert.strictEqual(r.status, 3, `stdout: ${r.stdout}\nstderr: ${r.stderr}`);
  assert.match(r.stderr, /Refusing to verify dev-fixture/);
});

test("production profile via env var: ceremony=dev-fixture → exit 3", () => {
  const dir = tmpDir();
  const manifest = makeStubArtifacts(dir, false);
  manifest.ceremony = { type: "dev-fixture" };
  const manifestPath = writeManifest(dir, manifest);
  const r = runScript(
    ["--manifest", manifestPath],
    { ANNEX_BUILD_PROFILE: "production" }
  );
  assert.strictEqual(r.status, 3, `stdout: ${r.stdout}\nstderr: ${r.stderr}`);
});

test("release alias maps to production profile", () => {
  const dir = tmpDir();
  const manifest = makeStubArtifacts(dir, false);
  manifest.ceremony = { type: "dev-fixture" };
  const manifestPath = writeManifest(dir, manifest);
  const r = runScript(
    ["--manifest", manifestPath],
    { ANNEX_BUILD_PROFILE: "release" }
  );
  assert.strictEqual(r.status, 3, `stdout: ${r.stdout}\nstderr: ${r.stderr}`);
});

test("production profile + ANNEX_ALLOW_DEV_CEREMONY=1 → success", () => {
  const dir = tmpDir();
  const manifest = makeStubArtifacts(dir, false);
  manifest.ceremony = { type: "dev-fixture" };
  const manifestPath = writeManifest(dir, manifest);
  const r = runScript(
    ["--manifest", manifestPath, "--profile", "production"],
    { ANNEX_ALLOW_DEV_CEREMONY: "1" }
  );
  assert.strictEqual(r.status, 0, `stderr: ${r.stderr}\nstdout: ${r.stdout}`);
  assert.match(r.stdout, /WARN ANNEX_ALLOW_DEV_CEREMONY=1/);
});

test("production profile: ceremony=mpc → success", () => {
  const dir = tmpDir();
  const manifest = makeStubArtifacts(dir, false);
  manifest.ceremony = {
    type: "mpc",
    contributors: 5,
    transcript: "https://example.com/transcript",
  };
  const manifestPath = writeManifest(dir, manifest);
  const r = runScript(["--manifest", manifestPath, "--profile", "production"]);
  assert.strictEqual(r.status, 0, `stderr: ${r.stderr}\nstdout: ${r.stdout}`);
  // No dev-fixture warning, no escape-hatch warning.
  assert.doesNotMatch(r.stdout, /dev-fixture/);
});

test("unknown profile → exit 1", () => {
  const dir = tmpDir();
  const manifest = makeStubArtifacts(dir, false);
  const manifestPath = writeManifest(dir, manifest);
  const r = runScript(["--manifest", manifestPath, "--profile", "produktion"]);
  assert.strictEqual(r.status, 1, `stdout: ${r.stdout}\nstderr: ${r.stderr}`);
  assert.match(r.stderr, /unrecognised build profile/);
});

test("ANNEX_ALLOW_DEV_CEREMONY=1 has no effect under dev profile", () => {
  // Sanity check: the opt-in shouldn't *down*-grade to a production check
  // in dev — dev should keep working as it always did.
  const dir = tmpDir();
  const manifest = makeStubArtifacts(dir, false);
  manifest.ceremony = { type: "dev-fixture" };
  const manifestPath = writeManifest(dir, manifest);
  const r = runScript(
    ["--manifest", manifestPath],
    { ANNEX_ALLOW_DEV_CEREMONY: "1", ANNEX_BUILD_PROFILE: "dev" }
  );
  assert.strictEqual(r.status, 0, `stderr: ${r.stderr}\nstdout: ${r.stdout}`);
  // The opt-in warning fires only under production — confirm it does NOT
  // show up here.
  assert.doesNotMatch(r.stdout, /WARN ANNEX_ALLOW_DEV_CEREMONY=1/);
});

test("hash mismatch → exit 2 (regression: still detected under production)", () => {
  const dir = tmpDir();
  const manifest = makeStubArtifacts(dir, false);
  manifest.ceremony = { type: "mpc" };
  // Corrupt one of the artifacts so its sha256 no longer matches.
  fs.writeFileSync(path.join(dir, "membership.wasm"), "tampered-bytes");
  const manifestPath = writeManifest(dir, manifest);
  const r = runScript(["--manifest", manifestPath, "--profile", "production"]);
  assert.strictEqual(r.status, 2, `stdout: ${r.stdout}\nstderr: ${r.stderr}`);
  assert.match(r.stderr, /MISMATCH\s+wasm/);
});

test("manifest missing required field → exit 1", () => {
  const dir = tmpDir();
  const manifest = makeStubArtifacts(dir, false);
  delete manifest.curve;
  const manifestPath = writeManifest(dir, manifest);
  const r = runScript(["--manifest", manifestPath]);
  assert.strictEqual(r.status, 1, `stdout: ${r.stdout}\nstderr: ${r.stderr}`);
  assert.match(r.stderr, /missing required field: curve/);
});
