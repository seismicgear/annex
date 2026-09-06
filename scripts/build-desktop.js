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
//                    pinned manifest for EVERY enabled circuit (membership,
//                    membership_v2, channel_eligibility, link_pseudonyms,
//                    federation_attestation). NEVER generates new keys.
//                    Hard-fails if any circuit's manifest is missing or its
//                    artifacts don't match the pinned hash — so a release can't
//                    bundle unverified v2/capability keys for the default
//                    identity path. Use for bundle / release builds.
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
  // Verify EVERY enabled circuit's pinned artifacts, not just membership.
  // The shipped client generates v2 proofs by default and the server accepts
  // v2 + the capability circuits, so a release that only gated membership could
  // bundle unverified (random/dev) v2/capability wasm/zkey/vkey — defeating the
  // pinned-ceremony gate for the DEFAULT identity path (AUDIT / Codex P1).
  const PRODUCTION_MANIFESTS = [
    "artifacts/membership/manifest.json",
    "artifacts/membership_v2/manifest.json",
    "artifacts/channel_eligibility/manifest.json",
    "artifacts/link_pseudonyms/manifest.json",
    "artifacts/federation_attestation/manifest.json",
  ];
  log("Verifying pinned ZK artifacts against per-circuit manifests...");
  for (const manifest of PRODUCTION_MANIFESTS) {
    if (!fs.existsSync(path.join(ZK_DIR, manifest))) {
      fatal(
        `${manifest} is missing. Production builds require a pinned manifest for every ` +
          `enabled circuit (the default identity path is v2). Add it before releasing ` +
          `— see docs/refactor/zk-merkle-production.md.`
      );
    }
    run(`node scripts/verify-artifacts.js --manifest ${manifest}`, ZK_DIR);
  }
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
  // v2 (secret-derived nullifier) — the shipped client generates v2 proofs by
  // default, so these must be served alongside v1.
  {
    label: "membership_v2.wasm",
    src: path.join(ZK_BUILD_DIR, "membership_v2_js", "membership_v2.wasm"),
    dst: path.join(CLIENT_PUBLIC_ZK, "membership_v2.wasm"),
  },
  {
    label: "membership_v2_final.zkey",
    src: path.join(ZK_KEYS_DIR, "membership_v2_final.zkey"),
    dst: path.join(CLIENT_PUBLIC_ZK, "membership_v2_final.zkey"),
  },
  // Capability / linkage / federation circuits (AUDIT P4-ID-1). The client
  // generates these proofs for role-gated channel access, opt-in pseudonym
  // linkage, and federated attestation.
  ...["channel_eligibility", "link_pseudonyms", "federation_attestation"].flatMap(
    (name) => [
      {
        label: `${name}.wasm`,
        src: path.join(ZK_BUILD_DIR, `${name}_js`, `${name}.wasm`),
        dst: path.join(CLIENT_PUBLIC_ZK, `${name}.wasm`),
      },
      {
        label: `${name}_final.zkey`,
        src: path.join(ZK_KEYS_DIR, `${name}_final.zkey`),
        dst: path.join(CLIENT_PUBLIC_ZK, `${name}_final.zkey`),
      },
    ]
  ),
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

// Belt-and-suspenders: the server-side verification key is shipped as a
// Tauri `bundle.resources` entry (see crates/annex-desktop/tauri.conf.json).
// If it's missing, Tauri's bundler will fail with a confusing "resource not
// found" error AFTER the long Cargo build. Fail here instead — fast and clear.
const SERVER_VKEY = path.join(ZK_KEYS_DIR, "membership_vkey.json");
if (!fs.existsSync(SERVER_VKEY)) {
  if (IS_PRODUCTION) {
    fatal(
      `${SERVER_VKEY} is missing. The desktop bundle requires it as a Tauri ` +
        `resource so the embedded server can verify membership proofs.`
    );
  } else {
    log(
      `  WARNING (dev): ${SERVER_VKEY} is missing. Tauri bundling will fail; ` +
        `desktop binary builds (cargo build) will fall back to the dummy vkey.`
    );
  }
}

// v2 server vkey — bundled as a Tauri resource (tauri.conf.json) and required
// at startup because the default `enabled_zk_versions` now includes "v2".
const SERVER_VKEY_V2 = path.join(ZK_KEYS_DIR, "membership_v2_vkey.json");
if (!fs.existsSync(SERVER_VKEY_V2)) {
  if (IS_PRODUCTION) {
    fatal(
      `${SERVER_VKEY_V2} is missing. The desktop bundle requires it as a Tauri ` +
        `resource so the embedded server can verify v2 (secret-derived nullifier) proofs.`
    );
  } else {
    log(
      `  WARNING (dev): ${SERVER_VKEY_V2} is missing. Tauri bundling will fail ` +
        `(membership_v2_vkey.json is a declared bundle resource).`
    );
  }
}

// Capability / linkage / federation server vkeys — bundled as Tauri resources
// (tauri.conf.json) and required at startup under enforcement so the embedded
// server can verify those proofs (else the endpoints 503).
for (const name of [
  "channel_eligibility",
  "link_pseudonyms",
  "federation_attestation",
]) {
  const vkey = path.join(ZK_KEYS_DIR, `${name}_vkey.json`);
  if (!fs.existsSync(vkey)) {
    if (IS_PRODUCTION) {
      fatal(
        `${vkey} is missing. The desktop bundle requires it as a Tauri resource ` +
          `so the embedded server can verify ${name} proofs.`
      );
    } else {
      log(
        `  WARNING (dev): ${vkey} is missing. Tauri bundling will fail ` +
          `(${name}_vkey.json is a declared bundle resource).`
      );
    }
  }
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
//
// Tauri on Windows uses \\?\ extended-length paths which don't follow NTFS
// reparse points (junctions/symlinks). Copying the dist here ensures Tauri
// finds it at ./dist without traversing any junctions. We also use plain
// path.join (no shell quoting) and fs.* primitives throughout so paths with
// spaces — e.g. `C:\Users\My Name\…` — round-trip correctly.
const CLIENT_DIST = path.join(CLIENT_DIR, "dist");
const TAURI_DIST = path.join(ROOT_DIR, "crates", "annex-desktop", "dist");

if (!fs.existsSync(path.join(CLIENT_DIST, "index.html"))) {
  fatal(`client dist not found at ${CLIENT_DIST} — vite build did not produce index.html`);
}

// Replace the Tauri dist atomically-ish: remove the old tree, then copy.
// rmSync({force:true}) so a partial copy from a previous interrupted run
// can't leave us deadlocked. maxRetries handles transient AV/indexer
// locks on Windows that briefly hold files open after the build finishes.
if (fs.existsSync(TAURI_DIST)) {
  fs.rmSync(TAURI_DIST, { recursive: true, force: true, maxRetries: 5, retryDelay: 100 });
}
fs.cpSync(CLIENT_DIST, TAURI_DIST, { recursive: true });

// Verify the copy actually produced the entrypoint Tauri will load.
if (!fs.existsSync(path.join(TAURI_DIST, "index.html"))) {
  fatal(
    `failed to copy client dist into ${TAURI_DIST} — index.html is missing after cpSync`
  );
}
log(`Copied client dist to ${TAURI_DIST}.`);

log("All done.");
