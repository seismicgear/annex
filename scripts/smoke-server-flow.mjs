#!/usr/bin/env node
//
// smoke-server-flow.mjs — Identity registration + membership verification
// flow against a running Annex server. Invoked by smoke-server.sh /
// smoke-server.ps1 once the server is up on `--url`.
//
// Steps (all required to claim the smoke is green):
//   1. POST /api/registry/register with a freshly generated sk + commitment.
//   2. Generate a Groth16 membership proof from the registration response,
//      using snarkjs + the membership.wasm + membership_final.zkey.
//   3. POST /api/zk/verify-membership; the server signs an HMAC session
//      token if and only if the proof verifies and matches the claimed
//      commitment.
//   4. POST /api/channels with the session token to confirm authenticated
//      writes go through.
//
// Skips the proof + downstream steps (with a non-zero exit code) only when
// the proving artifacts aren't present, since `enforce_zk_proofs=true` on
// the server would otherwise reject every authenticated request anyway.
//
// Usage:
//   node scripts/smoke-server-flow.mjs --url http://127.0.0.1:PORT

import { createRequire } from 'node:module';
import { existsSync, readFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { webcrypto } from 'node:crypto';

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '..');
const ZK_DIR = join(REPO_ROOT, 'zk');
const ZK_BUILD_DIR = join(ZK_DIR, 'build');
const ZK_KEYS_DIR = join(ZK_DIR, 'keys');
const MEMBERSHIP_WASM = join(ZK_BUILD_DIR, 'membership_js', 'membership.wasm');
const MEMBERSHIP_ZKEY = join(ZK_KEYS_DIR, 'membership_final.zkey');
const MEMBERSHIP_VKEY = join(ZK_KEYS_DIR, 'membership_vkey.json');

// Resolve snarkjs / circomlibjs through zk/node_modules.
const zkRequire = createRequire(join(ZK_DIR, 'package.json'));

const TOPIC = 'annex:identity:v1';
const TREE_DEPTH = 20;
// BN254 scalar field prime.
const FIELD_P = BigInt(
  '21888242871839275222246405745257275088548364400416034343698204186575808495617',
);

function parseArgs(argv) {
  const out = { url: null };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--url' && i + 1 < argv.length) {
      out.url = argv[++i];
    } else if (argv[i].startsWith('--url=')) {
      out.url = argv[i].slice('--url='.length);
    }
  }
  return out;
}

function step(msg) {
  console.log(`[smoke-flow] ${msg}`);
}

function fail(msg, err) {
  console.error(`[smoke-flow] FAIL: ${msg}`);
  if (err) {
    console.error(err.stack ?? err.message ?? String(err));
  }
  process.exit(1);
}

function randomScalar() {
  const bytes = new Uint8Array(32);
  webcrypto.getRandomValues(bytes);
  let n = 0n;
  for (const b of bytes) {
    n = (n << 8n) | BigInt(b);
  }
  n = n % FIELD_P;
  if (n === 0n) n = 1n;
  return n;
}

function randomNodeId() {
  const arr = new Uint32Array(1);
  webcrypto.getRandomValues(arr);
  return (arr[0] % 1_000_000) + 1;
}

function toHex64(value) {
  return value.toString(16).padStart(64, '0');
}

async function postJson(url, body, headers = {}) {
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...headers },
    body: JSON.stringify(body),
  });
  const text = await res.text();
  if (!res.ok) {
    throw new Error(`POST ${url} → ${res.status}: ${text}`);
  }
  return text.length > 0 ? JSON.parse(text) : {};
}

async function main() {
  const { url } = parseArgs(process.argv.slice(2));
  if (!url) {
    fail('--url <serverUrl> is required (e.g. http://127.0.0.1:7321)');
  }

  // ── 0. Tooling / artifact preflight ────────────────────────────────
  for (const [label, path] of [
    ['membership_vkey.json', MEMBERSHIP_VKEY],
    ['membership.wasm', MEMBERSHIP_WASM],
    ['membership_final.zkey', MEMBERSHIP_ZKEY],
  ]) {
    if (!existsSync(path)) {
      fail(
        `missing ZK artifact ${label} (${path}). Run \`(cd zk && npm ci && ` +
          `node scripts/build-circuits.js && node scripts/setup-groth16.js)\` first.`,
      );
    }
  }

  let snarkjs;
  let buildPoseidon;
  try {
    snarkjs = zkRequire('snarkjs');
    ({ buildPoseidon } = zkRequire('circomlibjs'));
  } catch (err) {
    fail(
      'snarkjs/circomlibjs not installed under zk/node_modules. ' +
        'Run `npm --prefix zk ci` first.',
      err,
    );
  }

  step(`server URL: ${url}`);

  // ── 1. /health ─────────────────────────────────────────────────────
  step('GET /health');
  const healthRes = await fetch(`${url}/health`);
  if (!healthRes.ok) {
    fail(`/health returned ${healthRes.status}: ${await healthRes.text()}`);
  }
  const health = await healthRes.json();
  if (health.status !== 'ok') {
    fail(`/health body did not report status=ok: ${JSON.stringify(health)}`);
  }
  step(`/health ok`);

  // ── 2. Build identity ──────────────────────────────────────────────
  step('generating identity (sk + commitment)');
  const poseidon = await buildPoseidon();
  const sk = randomScalar();
  const roleCode = 1; // Human
  const nodeId = randomNodeId();
  const commitmentField = poseidon.F.toObject(poseidon([sk, BigInt(roleCode), BigInt(nodeId)]));
  const commitmentHex = toHex64(commitmentField);
  step(`commitment = 0x${commitmentHex.slice(0, 16)}…`);

  // ── 3. POST /api/registry/register ─────────────────────────────────
  step('POST /api/registry/register');
  const registerResp = await postJson(`${url}/api/registry/register`, {
    commitmentHex,
    roleCode,
    nodeId,
  });
  if (
    typeof registerResp.identityId !== 'number' ||
    typeof registerResp.leafIndex !== 'number' ||
    typeof registerResp.rootHex !== 'string' ||
    !Array.isArray(registerResp.pathElements) ||
    !Array.isArray(registerResp.pathIndexBits)
  ) {
    fail(`unexpected register response shape: ${JSON.stringify(registerResp)}`);
  }
  const { identityId, leafIndex, rootHex, pathElements, pathIndexBits } = registerResp;
  step(`registered identityId=${identityId}, leafIndex=${leafIndex}`);

  // ── 4. GET /api/registry/path/{commitmentHex} ──────────────────────
  step(`GET /api/registry/path/${commitmentHex.slice(0, 8)}…`);
  const pathRes = await fetch(`${url}/api/registry/path/${commitmentHex}`);
  if (!pathRes.ok) {
    fail(`/api/registry/path returned ${pathRes.status}: ${await pathRes.text()}`);
  }
  const pathResp = await pathRes.json();
  if (
    pathResp.leafIndex !== leafIndex ||
    pathResp.rootHex !== rootHex ||
    pathResp.pathElements.length !== pathElements.length ||
    pathResp.pathIndexBits.length !== pathIndexBits.length
  ) {
    fail(
      'registration response and /api/registry/path disagree on the Merkle path',
    );
  }
  step(`merkle path matches register response (depth=${pathElements.length})`);

  // ── 5. Generate Groth16 membership proof ──────────────────────────
  step('generating Groth16 membership proof');
  const witnessInput = {
    sk: sk.toString(),
    roleCode: roleCode.toString(),
    nodeId: nodeId.toString(),
    leafIndex: leafIndex.toString(),
    pathElements: pathElements.map((s) => '0x' + s),
    pathIndexBits: pathIndexBits.map((b) => b.toString()),
  };
  const t0 = Date.now();
  const { proof, publicSignals } = await snarkjs.groth16.fullProve(
    witnessInput,
    MEMBERSHIP_WASM,
    MEMBERSHIP_ZKEY,
  );
  step(`proof generated in ${Date.now() - t0}ms`);
  if (publicSignals.length !== 2) {
    fail(`expected 2 public signals (root, commitment), got ${publicSignals.length}`);
  }
  if (pathElements.length !== TREE_DEPTH) {
    fail(`expected Merkle path of depth ${TREE_DEPTH}, got ${pathElements.length}`);
  }

  // Sanity: signal[1] (commitment) must match what we registered.
  const sigCommitmentHex = toHex64(BigInt(publicSignals[1]));
  if (sigCommitmentHex !== commitmentHex) {
    fail(
      `proof commitment ${sigCommitmentHex} does not match registered commitment ${commitmentHex}`,
    );
  }

  // ── 6. POST /api/zk/verify-membership ──────────────────────────────
  step('POST /api/zk/verify-membership');
  const verifyResp = await postJson(`${url}/api/zk/verify-membership`, {
    root: rootHex,
    commitment: commitmentHex,
    topic: TOPIC,
    proof,
    publicSignals,
  });
  if (verifyResp.ok !== true || typeof verifyResp.sessionToken !== 'string') {
    fail(`verify-membership did not issue a session token: ${JSON.stringify(verifyResp)}`);
  }
  const sessionToken = verifyResp.sessionToken;
  const pseudonymId = verifyResp.pseudonymId;
  step(`verified membership; pseudonym=${pseudonymId.slice(0, 16)}…`);

  // ── 7. GET /api/identity/{pseudonymId} ─────────────────────────────
  // The first identity registered against a fresh server is granted
  // founder capabilities by `create_platform_identity` during the
  // verify-membership step above. This call confirms that invariant
  // (and exercises `fetch_platform_identity`'s `ensure_founder`
  // self-heal path as a defence in depth) before the moderator-gated
  // POST /api/channels.
  step(`GET /api/identity/${pseudonymId.slice(0, 8)}…`);
  const identityRes = await fetch(`${url}/api/identity/${pseudonymId}`);
  if (!identityRes.ok) {
    fail(`/api/identity returned ${identityRes.status}: ${await identityRes.text()}`);
  }
  const identityBody = await identityRes.json();
  if (identityBody.capabilities?.can_moderate !== true) {
    fail(
      'expected the founder-promotion path to grant can_moderate=true to the ' +
        'first registered identity, got ' + JSON.stringify(identityBody.capabilities),
    );
  }
  step('founder-promotion confirmed (can_moderate=true)');

  // ── 8. POST /api/channels (authenticated) ──────────────────────────
  // The freshly registered identity is the founder, so it should have
  // can_moderate=true and be allowed to create channels.
  const channelId = `smoke-${Date.now().toString(36)}`;
  step(`POST /api/channels (channel_id=${channelId})`);
  const createRes = await fetch(`${url}/api/channels`, {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'authorization': `Bearer ${sessionToken}`,
    },
    body: JSON.stringify({
      channel_id: channelId,
      name: 'Smoke',
      channel_type: 'Text',
      topic: 'smoke test',
      vrp_topic_binding: null,
      required_capabilities_json: null,
      agent_min_alignment: null,
      retention_days: null,
      federation_scope: 'Local',
    }),
  });
  if (!createRes.ok) {
    const body = await createRes.text();
    fail(`POST /api/channels returned ${createRes.status}: ${body}`);
  }
  const createBody = await createRes.json();
  if (createBody.status !== 'created') {
    fail(`POST /api/channels did not return status=created: ${JSON.stringify(createBody)}`);
  }
  step(`channel created: ${channelId}`);

  // Quietly read back the verification key so the file we asked the
  // server to load is at least valid JSON we can parse on this side too.
  try {
    JSON.parse(readFileSync(MEMBERSHIP_VKEY, 'utf-8'));
  } catch (err) {
    fail('membership_vkey.json is not parseable JSON', err);
  }

  step('OK — full identity flow succeeded against enforce_zk_proofs=true server');
}

main().catch((err) => fail('unexpected error', err));
