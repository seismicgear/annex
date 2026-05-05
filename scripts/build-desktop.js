#!/usr/bin/env node
// build-desktop.js — Builds ZK artifacts, Piper TTS, and the client for Tauri
// desktop packaging.
//
// Cross-platform replacement for build-desktop.sh.
// Invoked by `tauri.conf.json`'s `beforeBuildCommand`.
//
// Usage:
//   node scripts/build-desktop.js                              # dev (default)
//   ANNEX_BUILD_PROFILE=production node scripts/build-desktop.js
//                                                              # production
//   SKIP_ZK=1 node scripts/build-desktop.js                    # skip ZK (dev only)
//   SKIP_PIPER=1 node scripts/build-desktop.js                 # skip Piper setup
//
// Profiles:
//   dev (default)  — generates ZK artifacts on demand via dev-setup-groth16.js,
//                    warns (does not fail) if a client artifact is missing
//                    after copy. Suitable for `cargo tauri dev` and local
//                    iteration. Random-entropy keys; NOT production-safe.
//
//   production     — runs `node zk/scripts/verify-artifacts.js` against the
//                    pinned manifest at zk/artifacts/membership/manifest.json.
//                    NEVER generates new keys. Hard-fails if any required
//                    client artifact (membership.wasm, membership_final.zkey)
//                    is missing or doesn't match the pinned hash. Use for
//                    bundle / release builds.
//
// See docs/refactor/zk-merkle-production.md.

const { execSync } = require("child_process");
const fs = require("fs");
const path = require("path");

const isWindows = process.platform === "win32";

const ROOT_DIR = execSync("git rev-parse --show-toplevel", { encoding: "utf-8" }).trim();
const ZK_DIR = path.join(ROOT_DIR, "zk");
const CLIENT_DIR = path.join(ROOT_DIR, "client");
const ZK_KEYS_DIR = path.join(ZK_DIR, "keys");
const ZK_BUILD_DIR = path.join(ZK_DIR, "build");
const CLIENT_PUBLIC_ZK = path.join(CLIENT_DIR, "public", "zk");

const ASSETS_DIR = path.join(ROOT_DIR, "assets");
const PIPER_DIR = path.join(ASSETS_DIR, "piper");
const VOICES_DIR = path.join(ASSETS_DIR, "voices");
const PIPER_BIN = path.join(PIPER_DIR, isWindows ? "piper.exe" : "piper");
const VOICE_MODEL = path.join(VOICES_DIR, "en_US-lessac-medium.onnx");

const PROFILE = (process.env.ANNEX_BUILD_PROFILE || "dev").trim().toLowerCase();
if (PROFILE !== "dev" && PROFILE !== "production" && PROFILE !== "release") {
  console.error(
    `[build-desktop] ERROR: ANNEX_BUILD_PROFILE=${process.env.ANNEX_BUILD_PROFILE} ` +
      `is not a recognised profile. Use "dev" or "production".`
  );
  process.exit(1);
}
const IS_PRODUCTION = PROFILE === "production" || PROFILE === "release";

function run(cmd, cwd) {
  console.log(`[build-desktop]   $ ${cmd}`);
  execSync(cmd, { cwd, stdio: "inherit" });
}

function log(msg) {
  console.log(`[build-desktop] ${msg}`);
}

function fatal(msg) {
  console.error(`[build-desktop] ERROR: ${msg}`);
  process.exit(1);
}

log(`profile: ${IS_PRODUCTION ? "production" : "dev"}`);

// ── Step 1: ZK artifacts — verify (production) or generate-on-demand (dev) ──

if (IS_PRODUCTION) {
  if (process.env.SKIP_ZK === "1") {
    fatal("SKIP_ZK=1 is forbidden in production builds; refusing to skip ZK verification.");
  }
  log("Verifying pinned ZK artifacts against manifest...");
  run("node scripts/verify-artifacts.js", ZK_DIR);
  log("ZK artifacts verified.");
} else if (process.env.SKIP_ZK === "1") {
  log("Skipping ZK build (SKIP_ZK=1, dev profile)");
} else if (
  fs.existsSync(path.join(ZK_KEYS_DIR, "membership_vkey.json")) &&
  fs.existsSync(path.join(ZK_KEYS_DIR, "membership_final.zkey")) &&
  fs.existsSync(
    path.join(ZK_BUILD_DIR, "membership_js", "membership.wasm")
  )
) {
  log("ZK artifacts already exist — skipping ZK build (dev)");
} else {
  log("Building ZK circuits (dev — random entropy)...");

  if (!fs.existsSync(path.join(ZK_DIR, "node_modules"))) {
    log("  Installing ZK dependencies...");
    run("npm ci", ZK_DIR);
  }

  log("  Compiling circuits...");
  run("node scripts/build-circuits.js", ZK_DIR);

  log("  Running Groth16 trusted setup (DEV-ONLY, random entropy)...");
  run("node scripts/dev-setup-groth16.js", ZK_DIR);

  log("ZK build complete (dev).");
}

// ── Step 2: Copy ZK client artifacts to client/public/zk/ ──
//
// In production, missing files are a hard error (verify-artifacts.js above
// already confirmed the source files exist and match the manifest). In dev,
// we still warn-and-continue so iteration without ZK is possible.

log("Copying ZK artifacts to client/public/zk/...");
fs.mkdirSync(CLIENT_PUBLIC_ZK, { recursive: true });

const clientArtifacts = [
  {
    label: "membership.wasm",
    src: path.join(ZK_BUILD_DIR, "membership_js", "membership.wasm"),
    dst: path.join(CLIENT_PUBLIC_ZK, "membership.wasm"),
  },
  {
    label: "membership_final.zkey",
    src: path.join(ZK_KEYS_DIR, "membership_final.zkey"),
    dst: path.join(CLIENT_PUBLIC_ZK, "membership_final.zkey"),
  },
];

for (const a of clientArtifacts) {
  if (!fs.existsSync(a.src)) {
    if (IS_PRODUCTION) {
      fatal(
        `${a.label} not found at ${a.src} in production build. ` +
          `verify-artifacts.js should have caught this; the manifest may be stale.`
      );
    } else {
      log(`  WARNING (dev): ${a.label} not found — client proof generation will fail`);
    }
    continue;
  }
  fs.copyFileSync(a.src, a.dst);
  log(`  Copied ${a.label}`);
}

// ── Step 3: Setup Piper TTS (download binary + voice model if missing) ──

if (process.env.SKIP_PIPER === "1") {
  log("Skipping Piper setup (SKIP_PIPER=1)");
} else if (fs.existsSync(PIPER_BIN) && fs.existsSync(VOICE_MODEL)) {
  log("Piper binary and voice model already exist — skipping setup");
} else {
  log("Setting up Piper TTS...");
  if (isWindows) {
    run(
      `powershell -ExecutionPolicy Bypass -File "${path.join(ROOT_DIR, "scripts", "setup-piper.ps1")}"`,
      ROOT_DIR
    );
  } else {
    run(
      `bash "${path.join(ROOT_DIR, "scripts", "setup-piper.sh")}"`,
      ROOT_DIR
    );
  }
  // Verify the setup actually produced the expected files
  if (!fs.existsSync(PIPER_BIN)) {
    console.error(
      `[build-desktop] ERROR: Piper binary not found at ${PIPER_BIN} after setup`
    );
    process.exit(1);
  }
  if (!fs.existsSync(VOICE_MODEL)) {
    console.error(
      `[build-desktop] ERROR: Voice model not found at ${VOICE_MODEL} after setup`
    );
    process.exit(1);
  }
  log("Piper setup complete.");
}

// ── Step 4: Build the client ──

log("Building client...");

if (!fs.existsSync(path.join(CLIENT_DIR, "node_modules"))) {
  log("  Installing client dependencies...");
  run("npm ci", CLIENT_DIR);
}

run("npm run build", CLIENT_DIR);
log("Client build complete.");

// Copy client dist into the Tauri project directory.
// Tauri on Windows uses \\?\ extended-length paths which don't follow NTFS
// reparse points (junctions/symlinks). Copying the dist here ensures Tauri
// finds it at ./dist without traversing any junctions.
const CLIENT_DIST = path.join(CLIENT_DIR, "dist");
const TAURI_DIST = path.join(ROOT_DIR, "crates", "annex-desktop", "dist");

if (!fs.existsSync(path.join(CLIENT_DIST, "index.html"))) {
  console.error(`[build-desktop] ERROR: client dist not found at ${CLIENT_DIST}`);
  process.exit(1);
}

if (fs.existsSync(TAURI_DIST)) {
  fs.rmSync(TAURI_DIST, { recursive: true });
}
fs.cpSync(CLIENT_DIST, TAURI_DIST, { recursive: true });
log("Copied client dist to Tauri project directory.");

log("All done.");
