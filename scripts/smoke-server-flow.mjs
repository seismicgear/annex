#!/usr/bin/env node
//
// smoke-server-flow.mjs — Identity registration + membership verification
// flow against a running Annex server. Invoked by smoke-server.sh /
// smoke-server.ps1 once the server is up on `--url`.
//
// Steps (all required to claim the smoke is green):
//   1. POST /api/registry/register with a freshly generated sk + commitment.
//   2. POST /api/zk/challenge for a single-use authentication challenge.
//   3. Generate a Groth16 membership_v2 proof from the registration response
//      and that challenge, using snarkjs + membership_v2.wasm +
//      membership_v2_final.zkey.
//   4. POST /api/zk/verify-membership; the server signs an HMAC session
//      token if and only if the proof verifies, matches the claimed
//      commitment, and spends a challenge it issued and has not yet seen.
//   5. REPLAY the byte-identical verify-membership body and require it to be
//      REFUSED. This is the smoke's security assertion, not a formality:
//      before the challenge existed every field of that body was stable for a
//      given member and topic, so a captured request was a bearer credential
//      that minted a fresh session at the identity's CURRENT revocation
//      epoch — i.e. revoking sessions did not survive re-authentication.
//   6. Revoke the identity's sessions, replay again (still refused), then
//      complete a FRESH challenge and proof and require that to succeed. A
//      fix that closed the replay by breaking legitimate re-authentication
//      would pass step 5 and fail here.
//   7. POST /api/channels with the session token to confirm authenticated
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
const MEMBERSHIP_WASM = join(ZK_BUILD_DIR, 'membership_v2_js', 'membership_v2.wasm');
const MEMBERSHIP_ZKEY = join(ZK_KEYS_DIR, 'membership_v2_final.zkey');
const MEMBERSHIP_VKEY = join(ZK_KEYS_DIR, 'membership_v2_vkey.json');

// Resolve snarkjs / circomlibjs through zk/node_modules.
const zkRequire = createRequire(join(ZK_DIR, 'package.json'));

// v2 topics are server-scoped, exactly as the client builds them in
// `client/src/stores/identity.ts`. The server recomputes `topicHash` from this
// string and rejects a proof bound to any other, so the two must agree
// literally.
const TOPIC = 'annex:server:smoke:v2';
// Must match `annex_identity::zk::topic_hash_for_v2` and the client's
// `computeTopicHashV2`: Fr::from_be_bytes_mod_order(SHA256(domain || topic)).
const V2_TOPIC_HASH_DOMAIN = 'annex/v2/topicHash:';
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

/** The v2 topicHash as a decimal field element, matching the server exactly. */
async function topicHashV2(topic) {
  const bytes = new TextEncoder().encode(V2_TOPIC_HASH_DOMAIN + topic);
  const digest = new Uint8Array(await webcrypto.subtle.digest('SHA-256', bytes));
  let n = 0n;
  for (const b of digest) n = (n << 8n) | BigInt(b);
  return n % FIELD_P;
}

/**
 * POST and return the status alongside the body, WITHOUT throwing.
 *
 * `postJson` below throws on any non-2xx, which is right for the happy path
 * and useless for the replay assertions — there the refusal IS the result
 * being measured, and a helper that turns it into an exception makes the
 * interesting case indistinguishable from a broken server.
 */
async function postJsonRaw(url, body, headers = {}) {
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...headers },
    body: JSON.stringify(body),
  });
  const text = await res.text();
  let parsed = null;
  try {
    parsed = text.length > 0 ? JSON.parse(text) : null;
  } catch {
    parsed = null;
  }
  return { status: res.status, text, body: parsed };
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
    ['membership_v2_vkey.json', MEMBERSHIP_VKEY],
    ['membership_v2.wasm', MEMBERSHIP_WASM],
    ['membership_v2_final.zkey', MEMBERSHIP_ZKEY],
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

  // ── 5. Sign in: challenge → proof → verify ────────────────────────
  //
  // Factored into a closure because the flow signs in THREE times below —
  // once legitimately, once after a revocation, and the replay attempts in
  // between reuse a captured body. Writing it once means the "a legitimate
  // holder can still sign in" assertions exercise the same code path as the
  // first sign-in rather than a second copy that could drift from it.
  const topicHash = await topicHashV2(TOPIC);
  if (pathElements.length !== TREE_DEPTH) {
    fail(`expected Merkle path of depth ${TREE_DEPTH}, got ${pathElements.length}`);
  }

  async function buildVerifyBody(label) {
    step(`${label}: POST /api/zk/challenge`);
    const chal = await postJson(`${url}/api/zk/challenge`, {
      commitment: commitmentHex,
      topic: TOPIC,
    });
    if (typeof chal.challenge !== 'string' || !/^[0-9a-f]{64}$/.test(chal.challenge)) {
      fail(`/api/zk/challenge returned an unusable challenge: ${JSON.stringify(chal)}`);
    }

    step(`${label}: generating Groth16 membership_v2 proof`);
    const t0 = Date.now();
    const { proof, publicSignals } = await snarkjs.groth16.fullProve(
      {
        sk: sk.toString(),
        roleCode: roleCode.toString(),
        nodeId: nodeId.toString(),
        leafIndex: leafIndex.toString(),
        pathElements: pathElements.map((s) => '0x' + s),
        pathIndexBits: pathIndexBits.map((b) => b.toString()),
        topicHash: topicHash.toString(),
        challenge: BigInt('0x' + chal.challenge).toString(),
      },
      MEMBERSHIP_WASM,
      MEMBERSHIP_ZKEY,
    );
    step(`${label}: proof generated in ${Date.now() - t0}ms`);

    // [root, commitment, nullifier, topicHash, challenge]
    if (publicSignals.length !== 5) {
      fail(
        `expected 5 public signals (root, commitment, nullifier, topicHash, ` +
          `challenge), got ${publicSignals.length}`,
      );
    }
    const sigCommitmentHex = toHex64(BigInt(publicSignals[1]));
    if (sigCommitmentHex !== commitmentHex) {
      fail(
        `proof commitment ${sigCommitmentHex} does not match registered commitment ${commitmentHex}`,
      );
    }
    // The challenge really is inside the proof. If this ever fails, the
    // circuit stopped binding it and every assertion below would be vacuous.
    if (toHex64(BigInt(publicSignals[4])) !== chal.challenge) {
      fail(
        `the proof's challenge signal (${toHex64(BigInt(publicSignals[4]))}) is not the ` +
          `challenge the server issued (${chal.challenge}) — the circuit is not binding it`,
      );
    }

    return {
      root: rootHex,
      commitment: commitmentHex,
      topic: TOPIC,
      proof,
      publicSignals,
      protocolVersion: 'v2',
      nullifierHex: toHex64(BigInt(publicSignals[2])),
      topicHashHex: toHex64(BigInt(publicSignals[3])),
      challengeHex: chal.challenge,
    };
  }

  // ── 6. POST /api/zk/verify-membership ──────────────────────────────
  const firstBody = await buildVerifyBody('sign-in');
  step('POST /api/zk/verify-membership');
  const verifyResp = await postJson(`${url}/api/zk/verify-membership`, firstBody);
  if (verifyResp.ok !== true || typeof verifyResp.sessionToken !== 'string') {
    fail(`verify-membership did not issue a session token: ${JSON.stringify(verifyResp)}`);
  }
  let sessionToken = verifyResp.sessionToken;
  const pseudonymId = verifyResp.pseudonymId;
  step(`verified membership; pseudonym=${pseudonymId.slice(0, 16)}…`);

  // ── 6a. The captured body must not sign in a second time ───────────
  step('REPLAY: re-POST the identical verify-membership body');
  const replay1 = await postJsonRaw(`${url}/api/zk/verify-membership`, firstBody);
  if (replay1.status < 400) {
    fail(
      `a byte-identical verify-membership body was accepted a second time ` +
        `(HTTP ${replay1.status}). The request is a bearer credential: anyone who ` +
        `captures it can mint sessions for this identity without holding sk.`,
    );
  }
  step(`replay refused with HTTP ${replay1.status} (expected)`);

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

  // ── 9. Revocation must survive re-authentication ───────────────────
  //
  // The defect this closes: revoking an identity's sessions bumps its token
  // epoch, which invalidates outstanding tokens — but re-authentication mints
  // a NEW token at the new epoch, and re-authentication took a request body
  // that was entirely replayable. So whoever held a captured sign-in could
  // simply present it again and walk back in past the revocation, never having
  // held `sk`. Revocation was undone by its own recovery path.
  //
  // This member is the founder, so it can revoke itself — which is all this
  // assertion needs and avoids provisioning a second identity.
  step(`POST /api/admin/members/${pseudonymId.slice(0, 8)}…/revoke-sessions`);
  const revokeRes = await fetch(
    `${url}/api/admin/members/${pseudonymId}/revoke-sessions`,
    {
      method: 'POST',
      headers: {
        'content-type': 'application/json',
        authorization: `Bearer ${sessionToken}`,
      },
      body: '{}',
    },
  );
  if (!revokeRes.ok) {
    fail(`revoke-sessions returned ${revokeRes.status}: ${await revokeRes.text()}`);
  }
  const revokeBody = await revokeRes.json();
  step(`sessions revoked; token_epoch now ${revokeBody.token_epoch}`);

  // The old token must be dead.
  const staleRes = await fetch(`${url}/api/channels`, {
    headers: { authorization: `Bearer ${sessionToken}` },
  });
  if (staleRes.status !== 401 && staleRes.status !== 403) {
    fail(
      `a token from before the revocation was still accepted (HTTP ${staleRes.status}) — ` +
        'the epoch bump is not being enforced',
    );
  }
  step(`pre-revocation token refused with HTTP ${staleRes.status} (expected)`);

  // And the captured sign-in must not mint a replacement.
  step('REPLAY after revocation: re-POST the captured verify-membership body');
  const replay2 = await postJsonRaw(`${url}/api/zk/verify-membership`, firstBody);
  if (replay2.status < 400) {
    fail(
      `the captured sign-in minted a session AFTER revocation (HTTP ${replay2.status}). ` +
        'Revocation does not survive its own re-authentication path.',
    );
  }
  step(`post-revocation replay refused with HTTP ${replay2.status} (expected)`);

  // ── 10. The legitimate holder must still be able to sign in ────────
  //
  // Without this the closure above could be satisfied by simply breaking
  // re-authentication, which is a worse defect than the one being fixed: an
  // enrolled member who cannot get back in has been locked out of a server
  // they belong to.
  const secondBody = await buildVerifyBody('re-auth');
  const reauth = await postJsonRaw(`${url}/api/zk/verify-membership`, secondBody);
  if (reauth.status !== 200 || typeof reauth.body?.sessionToken !== 'string') {
    fail(
      `a legitimate holder could not re-authenticate with a fresh challenge ` +
        `(HTTP ${reauth.status}): ${reauth.text}`,
    );
  }
  if (reauth.body.pseudonymId !== pseudonymId) {
    fail(
      `re-authentication resolved to a different pseudonym (${reauth.body.pseudonymId} ` +
        `vs ${pseudonymId}) — the nullifier is no longer deterministic`,
    );
  }
  sessionToken = reauth.body.sessionToken;
  step('re-authentication with a fresh challenge succeeded, same pseudonym');

  // The new token works against the same authenticated route the old one did.
  const postRevokeRes = await fetch(`${url}/api/channels`, {
    headers: { authorization: `Bearer ${sessionToken}` },
  });
  if (!postRevokeRes.ok) {
    fail(
      `the freshly minted token was refused (HTTP ${postRevokeRes.status}): ` +
        (await postRevokeRes.text()),
    );
  }
  step('post-revocation session token is accepted');

  // Quietly read back the verification key so the file we asked the
  // server to load is at least valid JSON we can parse on this side too.
  try {
    JSON.parse(readFileSync(MEMBERSHIP_VKEY, 'utf-8'));
  } catch (err) {
    fail('membership_vkey.json is not parseable JSON', err);
  }

  step(
    'OK — full identity flow succeeded against enforce_zk_proofs=true server, ' +
      'and a captured sign-in could not be replayed before or after revocation',
  );
}

// snarkjs's `groth16.fullProve` spins up a global BN128 curve worker-thread
// pool (`globalThis.curve_bn128`) that it never tears down. Those threads keep
// Node's event loop alive indefinitely, so after `main()` resolves the process
// would otherwise hang forever instead of exiting — which is exactly what made
// the Linux server-smoke CI job run until the 6-hour job timeout. Exit
// explicitly on success; `fail()` already exits non-zero on every error path.
main()
  .then(() => process.exit(0))
  .catch((err) => fail('unexpected error', err));
