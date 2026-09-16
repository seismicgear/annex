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
const crypto = require('crypto');
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

// v2 (secret-derived nullifier) circuit artifacts. The client generates v2
// proofs by default; these must be served alongside v1.
const wasmV2Source = path.join(ZK_DIR, 'build', 'membership_v2_js', 'membership_v2.wasm');
const zkeyV2Source = path.join(ZK_DIR, 'keys', 'membership_v2_final.zkey');
const wasmV2Dest = path.join(CLIENT_DIR, 'public', 'zk', 'membership_v2.wasm');
const zkeyV2Dest = path.join(CLIENT_DIR, 'public', 'zk', 'membership_v2_final.zkey');

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

// Every source artifact the dev client needs. The default registration path is
// now v2 (`generateMembershipProofV2`), so a checkout that has only the v1
// source must STILL rebuild — otherwise dev boots with no `/zk/membership_v2.*`
// and identity registration fails at proof generation. The capability circuits
// are included too so their endpoints aren't silently 503 in dev.
function requiredSourceArtifacts() {
  const list = [
    { label: 'membership.wasm', path: wasmSource },
    { label: 'membership_final.zkey', path: zkeySource },
    { label: 'membership_v2.wasm', path: wasmV2Source },
    { label: 'membership_v2_final.zkey', path: zkeyV2Source },
  ];
  for (const name of CAPABILITY_CIRCUITS) {
    const p = capabilityArtifactPaths(name);
    list.push({ label: `${name}.wasm`, path: p.wasmSource });
    list.push({ label: `${name}_final.zkey`, path: p.zkeySource });
  }
  return list;
}

function ensureSourceArtifacts() {
  const required = requiredSourceArtifacts();
  const missing = required.filter((a) => !exists(a.path));
  if (missing.length === 0) {
    log('ZK source artifacts already exist — skipping rebuild.');
    return;
  }

  warn('Missing ZK source artifacts required for desktop dev:');
  for (const a of missing) {
    warn(`  Missing ${a.label}: ${a.path}`);
  }
  log('Building ZK artifacts (one-time, may take a while)...');

  if (!exists(path.join(ZK_DIR, 'node_modules'))) {
    log('Installing zk dependencies...');
    run('npm ci', ZK_DIR);
  }

  run('node scripts/build-circuits.js', ZK_DIR);
  run('node scripts/dev-setup-groth16.js', ZK_DIR);

  const stillMissing = requiredSourceArtifacts().filter((a) => !exists(a.path));
  if (stillMissing.length > 0) {
    fail(
      'ZK build completed but required artifacts are still missing (' +
        stillMissing.map((a) => a.label).join(', ') +
        '). Check zk/scripts output above.'
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

  // v2 artifacts (best-effort: warn rather than fail so a v1-only checkout
  // still works, but the default client path is v2 so this should be present).
  if (exists(wasmV2Source) && exists(zkeyV2Source)) {
    fs.copyFileSync(wasmV2Source, wasmV2Dest);
    fs.copyFileSync(zkeyV2Source, zkeyV2Dest);
    log('Prepared client/public/zk v2 artifacts.');
  } else {
    warn('membership_v2 artifacts missing in zk/ — v2 proofs would fail.');
    warn(`Expected: ${wasmV2Source}`);
    warn(`Expected: ${zkeyV2Source}`);
  }

  // Capability / linkage / federation circuits (AUDIT P4-ID-1). Best-effort:
  // these endpoints return 503 if absent, so a checkout without them still
  // boots — but the default client ships them.
  copyCapabilityCircuits();

  log('Prepared client/public/zk artifacts for desktop dev.');
}

// Capability / linkage / federation circuits (AUDIT P4-ID-1).
const CAPABILITY_CIRCUITS = ['channel_eligibility', 'link_pseudonyms', 'federation_attestation'];

function capabilityArtifactPaths(name) {
  return {
    wasmSource: path.join(ZK_DIR, 'build', `${name}_js`, `${name}.wasm`),
    zkeySource: path.join(ZK_DIR, 'keys', `${name}_final.zkey`),
    wasmDest: path.join(CLIENT_DIR, 'public', 'zk', `${name}.wasm`),
    zkeyDest: path.join(CLIENT_DIR, 'public', 'zk', `${name}_final.zkey`),
  };
}

function copyCapabilityCircuits() {
  for (const name of CAPABILITY_CIRCUITS) {
    const p = capabilityArtifactPaths(name);
    if (exists(p.wasmSource) && exists(p.zkeySource)) {
      fs.mkdirSync(path.dirname(p.wasmDest), { recursive: true });
      fs.copyFileSync(p.wasmSource, p.wasmDest);
      fs.copyFileSync(p.zkeySource, p.zkeyDest);
      log(`Prepared client/public/zk ${name} artifacts.`);
    } else {
      warn(`${name} artifacts missing in zk/ — that circuit's endpoint will 503.`);
    }
  }
}

function capabilityArtifactsPresent() {
  return CAPABILITY_CIRCUITS.every((name) => {
    const p = capabilityArtifactPaths(name);
    return exists(p.wasmDest) && exists(p.zkeyDest);
  });
}

// ── Staleness is about CONTENT, not existence ─────────────────────────────
//
// This block used to be `if (every dest exists) { nothing to do }`, and that
// is wrong in the one case that matters. The client proves with the zkey it
// finds in `client/public/zk`; the server verifies with the vkey in
// `zk/keys`. Rotate the keys — a trusted-setup ceremony, a rebuild, a
// rebase — and the destinations still EXIST, so this script declared victory
// and left the client proving against the old proving key. Every proof then
// failed verification and the user saw `invalid proof` on the very first
// screen, with no indication that the two halves had drifted.
//
// It is the defect class CLAUDE.md already names: a fixture identified by a
// PATH rather than by a FILE. The ceremony made it real — the audit's founder
// setup failed with `invalid proof` and 0 of 104 surfaces ran.
//
// Hashing eight files costs ~50ms against a step that otherwise rebuilds
// circuits.
function sha256(filePath) {
  return crypto.createHash('sha256').update(fs.readFileSync(filePath)).digest('hex');
}

/** Source → destination pairs the dev client needs, in copy order. */
function artifactPairs() {
  const pairs = [
    { label: 'membership.wasm', from: wasmSource, to: wasmDest },
    { label: 'membership_final.zkey', from: zkeySource, to: zkeyDest },
    { label: 'membership_v2.wasm', from: wasmV2Source, to: wasmV2Dest },
    { label: 'membership_v2_final.zkey', from: zkeyV2Source, to: zkeyV2Dest },
  ];
  for (const name of CAPABILITY_CIRCUITS) {
    const p = capabilityArtifactPaths(name);
    pairs.push({ label: `${name}.wasm`, from: p.wasmSource, to: p.wasmDest });
    pairs.push({ label: `${name}_final.zkey`, from: p.zkeySource, to: p.zkeyDest });
  }
  return pairs;
}

/**
 * Destinations that are missing, or whose bytes differ from their source.
 * A source that does not exist yet is not "stale" — `ensureSourceArtifacts`
 * builds it first.
 */
function staleArtifacts() {
  const stale = [];
  for (const pair of artifactPairs()) {
    if (!exists(pair.from)) continue;
    if (!exists(pair.to)) {
      stale.push({ ...pair, reason: 'missing' });
      continue;
    }
    if (sha256(pair.from) !== sha256(pair.to)) {
      stale.push({ ...pair, reason: 'differs from zk/' });
    }
  }
  return stale;
}

const sourcesReady = requiredSourceArtifacts().every((a) => exists(a.path));
if (sourcesReady) {
  const stale = staleArtifacts();
  if (stale.length === 0) {
    log('client/public/zk artifacts match zk/. Nothing to do.');
    process.exit(0);
  }
  // Name each one. "Copying artifacts" tells an operator nothing; "the
  // proving key differs from the one in zk/keys" tells them a rotation
  // landed and this is the step that propagates it.
  warn(`${stale.length} client ZK artifact(s) are out of date:`);
  for (const a of stale) {
    warn(`  ${a.label}: ${a.reason}`);
  }
  copyArtifactsToClient();
  const remaining = staleArtifacts();
  if (remaining.length > 0) {
    fail(
      'client/public/zk is still out of sync after copying (' +
        remaining.map((a) => a.label).join(', ') +
        '). The dev client would prove against a key the server does not verify.'
    );
  }
  log('client/public/zk artifacts refreshed from zk/.');
  process.exit(0);
}

warn('Required ZK source artifacts are missing.');
warn(`Expected: ${zkeySource}`);
warn(`Expected: ${zkeyV2Source}`);

ensureSourceArtifacts();
copyArtifactsToClient();
const remaining = staleArtifacts();
if (remaining.length > 0) {
  fail(
    'client/public/zk is still out of sync after building and copying (' +
      remaining.map((a) => a.label).join(', ') +
      ').'
  );
}
