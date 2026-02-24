#!/usr/bin/env node
// build-desktop.js — Builds ZK artifacts and the client for Tauri desktop packaging.
//
// Cross-platform replacement for build-desktop.sh.
// Invoked by `tauri.conf.json`'s `beforeBuildCommand`.
//
// Usage:
//   node scripts/build-desktop.js            # full build (ZK + client)
//   SKIP_ZK=1 node scripts/build-desktop.js  # skip ZK, client build only

const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const ROOT_DIR = path.resolve(__dirname, "..");
const ZK_DIR = path.join(ROOT_DIR, "zk");
const CLIENT_DIR = path.join(ROOT_DIR, "client");
const ZK_KEYS_DIR = path.join(ZK_DIR, "keys");
const ZK_BUILD_DIR = path.join(ZK_DIR, "build");
const CLIENT_PUBLIC_ZK = path.join(CLIENT_DIR, "public", "zk");

function run(cmd, cwd) {
  console.log(`[build-desktop]   $ ${cmd}`);
  execSync(cmd, { cwd, stdio: "inherit" });
}

function log(msg) {
  console.log(`[build-desktop] ${msg}`);
}

// ── Step 1: Build ZK circuits (if not already built or SKIP_ZK is set) ──

if (process.env.SKIP_ZK === "1") {
  log("Skipping ZK build (SKIP_ZK=1)");
} else if (
  fs.existsSync(path.join(ZK_KEYS_DIR, "membership_vkey.json")) &&
  fs.existsSync(path.join(ZK_KEYS_DIR, "membership_final.zkey")) &&
  fs.existsSync(
    path.join(ZK_BUILD_DIR, "membership_js", "membership.wasm")
  )
) {
  log("ZK artifacts already exist — skipping ZK build");
} else {
  log("Building ZK circuits...");

  if (!fs.existsSync(path.join(ZK_DIR, "node_modules"))) {
    log("  Installing ZK dependencies...");
    run("npm ci", ZK_DIR);
  }

  log("  Compiling circuits...");
  run("node scripts/build-circuits.js", ZK_DIR);

  log("  Running Groth16 trusted setup...");
  run("node scripts/setup-groth16.js", ZK_DIR);

  log("ZK build complete.");
}

// ── Step 2: Copy ZK client artifacts to client/public/zk/ ──

log("Copying ZK artifacts to client/public/zk/...");
fs.mkdirSync(CLIENT_PUBLIC_ZK, { recursive: true });

const wasmSrc = path.join(ZK_BUILD_DIR, "membership_js", "membership.wasm");
if (fs.existsSync(wasmSrc)) {
  fs.copyFileSync(wasmSrc, path.join(CLIENT_PUBLIC_ZK, "membership.wasm"));
  log("  Copied membership.wasm");
} else {
  log(
    "  WARNING: membership.wasm not found — client proof generation will fail"
  );
}

const zkeySrc = path.join(ZK_KEYS_DIR, "membership_final.zkey");
if (fs.existsSync(zkeySrc)) {
  fs.copyFileSync(
    zkeySrc,
    path.join(CLIENT_PUBLIC_ZK, "membership_final.zkey")
  );
  log("  Copied membership_final.zkey");
} else {
  log(
    "  WARNING: membership_final.zkey not found — client proof generation will fail"
  );
}

// ── Step 3: Build the client ──

log("Building client...");

if (!fs.existsSync(path.join(CLIENT_DIR, "node_modules"))) {
  log("  Installing client dependencies...");
  run("npm ci", CLIENT_DIR);
}

run("npm run build", CLIENT_DIR);
log("Client build complete.");

log("All done.");
