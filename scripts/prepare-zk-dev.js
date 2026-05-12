#!/usr/bin/env node
//
// prepare-zk-dev.js — DEV-ONLY ZK artifact prep for `cargo tauri dev` and
// the standalone `npm --prefix client run dev` flow.
//
// Generates random-entropy artifacts via zk/scripts/dev-setup-groth16.js if
// they are missing, then copies the wasm + zkey into client/public/zk/ so
// the Vite dev server can serve them to the proof worker.
//
// Refuses to run when ANNEX_BUILD_PROFILE=production|release. Production
// builds verify pinned artifacts via scripts/build-desktop.js; this script
// must never be on a production build path. See
// docs/refactor/zk-merkle-production.md.

const { execSync } = require('child_process');
const fs = require('fs');
const path = require('path');

const profile = (process.env.ANNEX_BUILD_PROFILE || '').trim().toLowerCase();
if (profile === 'production' || profile === 'release') {
  process.stderr.write(
    `[zk-prep] REFUSING to run: ANNEX_BUILD_PROFILE=${process.env.ANNEX_BUILD_PROFILE}.\n` +
      `[zk-prep] prepare-zk-dev.js generates random-entropy keys and is dev-only.\n` +
      `[zk-prep] For production, run \`node zk/scripts/verify-artifacts.js\` and\n` +
      `[zk-prep] \`ANNEX_BUILD_PROFILE=production node scripts/build-desktop.js\`.\n`
  );
  process.exit(1);
}

const ROOT_DIR = execSync('git rev-parse --show-toplevel', { encoding: 'utf-8' }).trim();
const ZK_DIR = path.join(ROOT_DIR, 'zk');
const CLIENT_DIR = path.join(ROOT_DIR, 'client');

const wasmSource = path.join(ZK_DIR, 'build', 'membership_js', 'membership.wasm');
const zkeySource = path.join(ZK_DIR, 'keys', 'membership_final.zkey');
const wasmDest = path.join(CLIENT_DIR, 'public', 'zk', 'membership.wasm');
const zkeyDest = path.join(CLIENT_DIR, 'public', 'zk', 'membership_final.zkey');

function log(msg) {
  console.log(`[zk-prep] ${msg}`);
}

function warn(msg) {
  console.warn(`[zk-prep] WARNING: ${msg}`);
}

function fail(msg) {
  console.error(`[zk-prep] ERROR: ${msg}`);
  process.exit(1);
}

function run(cmd, cwd) {
  log(`$ ${cmd}`);
  execSync(cmd, { cwd, stdio: 'inherit' });
}

function exists(filePath) {
  return fs.existsSync(filePath);
}

function ensureSourceArtifacts() {
  if (exists(wasmSource) && exists(zkeySource)) {
    log('ZK source artifacts already exist — skipping rebuild.');
    return;
  }

  warn('Missing ZK source artifacts required for desktop dev.');
  warn(`Expected: ${wasmSource}`);
  warn(`Expected: ${zkeySource}`);
  log('Building ZK artifacts (one-time, may take a while)...');

  if (!exists(path.join(ZK_DIR, 'node_modules'))) {
    log('Installing zk dependencies...');
    run('npm ci', ZK_DIR);
  }

  run('node scripts/build-circuits.js', ZK_DIR);
  run('node scripts/dev-setup-groth16.js', ZK_DIR);

  if (!exists(wasmSource) || !exists(zkeySource)) {
    fail(
      'ZK build completed but required artifacts are still missing. Check zk/scripts output above.'
    );
  }
}

function copyArtifactsToClient() {
  fs.mkdirSync(path.dirname(wasmDest), { recursive: true });

  if (!exists(wasmDest)) {
    warn('client/public/zk/membership.wasm is missing. Copying from zk/build...');
  }
  fs.copyFileSync(wasmSource, wasmDest);

  if (!exists(zkeyDest)) {
    warn('client/public/zk/membership_final.zkey is missing. Copying from zk/keys...');
  }
  fs.copyFileSync(zkeySource, zkeyDest);

  if (!exists(wasmDest) || !exists(zkeyDest)) {
    fail(
      'Failed to prepare client/public/zk artifacts. Dev server would fail to generate proofs.'
    );
  }

  log('Prepared client/public/zk artifacts for desktop dev.');
}

if (exists(wasmDest) && exists(zkeyDest)) {
  log('client/public/zk artifacts already exist. Nothing to do.');
  process.exit(0);
}

warn('Required ZK artifacts for the dev client are missing.');
warn(`Expected: ${wasmDest}`);
warn(`Expected: ${zkeyDest}`);

ensureSourceArtifacts();
copyArtifactsToClient();
