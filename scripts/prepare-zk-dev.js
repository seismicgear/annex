#!/usr/bin/env node

const { execSync } = require('child_process');
const fs = require('fs');
const path = require('path');

const ROOT_DIR = path.resolve(__dirname, '..');
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
  run('node scripts/setup-groth16.js', ZK_DIR);

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
