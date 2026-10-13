// Real protocol acceptance helpers. No auth bypasses or synthetic server responses.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { createHash, randomBytes, randomInt, createPrivateKey, createPublicKey, sign } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../../', import.meta.url));
const requireZk = createRequire(new URL('../../zk/package.json', import.meta.url));
const { groth16 } = requireZk('snarkjs');
const { buildPoseidon } = requireZk('circomlibjs');
const poseidon = await buildPoseidon();
const prime = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
const proofHeaders = new Map();
const authenticatedIdentities = new Map();
const renewals = new Map();
export const hex = value => BigInt(value).toString(16).padStart(64, '0');
export const digest = text => createHash('sha256').update(text).digest('hex');
// Returned only to in-process acceptance callers; never put these headers in
// evidence. Binary multipart requests need the same genuine session assertion.
export function sessionHeaders(token) {
  assert(proofHeaders.has(token), 'A genuinely authenticated live identity is required');
  return { authorization: `Bearer ${token}`, 'x-annex-zk-proof': proofHeaders.get(token) };
}
async function renewRetiredProof(url, token, observed) {
  const identity = authenticatedIdentities.get(token);
  assert(identity && identity.token === token && new URL(identity.url).origin === new URL(url).origin,
    'Retired-root renewal must retain the exact authenticated identity, origin and bearer');
  if (proofHeaders.get(token) !== observed) return;
  if (!renewals.has(token)) {
    const work = (async () => {
      const prior = JSON.parse(Buffer.from(observed, 'base64').toString('utf8'));
      assert.equal(prior.protocolVersion, 'v2'); assert.equal(prior.publicSignals.length, 5);
      const proof = await prove(identity, hex(prior.publicSignals[4]));
      for (const index of [1, 2, 3, 4]) assert.equal(hex(proof.publicSignals[index]), hex(prior.publicSignals[index]),
        'Renewal may change only the current membership root');
      assert.equal(identity.token, token, 'A changed session cannot be resurrected by proof renewal');
      assert.equal(prior.commitment_hex, identity.commitment); assert.equal(prior.topic, identity.topic);
      if (proofHeaders.get(token) !== observed) return;
      proofHeaders.set(token, Buffer.from(JSON.stringify({ ...prior, proof: proof.proof,
        root_hex: proof.root, publicSignals: proof.publicSignals })).toString('base64'));
    })();
    renewals.set(token, work);
    void work.finally(() => { if (renewals.get(token) === work) renewals.delete(token); }).catch(() => {});
  }
  await renewals.get(token);
}
export async function request(url, route, { method = 'GET', body, token, expected = 200 } = {}) {
  const observed = token ? proofHeaders.get(token) : null;
  const perform = () => fetch(new URL(route, url), {
    method, headers: { 'content-type': 'application/json', ...(token ? {
      authorization: `Bearer ${token}`, ...(proofHeaders.has(token) ? { 'x-annex-zk-proof': proofHeaders.get(token) } : {}),
    } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body), signal: AbortSignal.timeout(30_000),
  });
  let response = await perform();
  // A generic denial/revocation is terminal. Retry once only when the actual
  // server proves a valid assertion names a known retired published root.
  if (response.status === 403 && token && observed) {
    const refusal = await response.clone().json().catch(() => null);
    if (refusal?.error === 'zk_root_retired') {
      await renewRetiredProof(url, token, observed);
      response = await perform();
    }
  }
  const text = await response.text();
  // Successful responses may contain credentials. Never include them in failure diagnostics.
  assert.equal(response.status, expected, `${method} ${route}: HTTP ${response.status}${!response.ok ? ` ${text}` : ''}`);
  return text ? JSON.parse(text) : null;
}
export async function register(url, roleCode = 1, topic = 'annex:acceptance:v2') {
  const sk = BigInt(`0x${randomBytes(32).toString('hex')}`) % (prime - 1n) + 1n;
  const nodeId = randomInt(1, 1_000_000);
  const commitment = hex(poseidon.F.toObject(poseidon([sk, BigInt(roleCode), BigInt(nodeId)])));
  await request(url, '/api/registry/register', { method: 'POST', body: { commitmentHex: commitment, roleCode, nodeId } });
  const identity = { url, sk, nodeId, commitment, roleCode, topic };
  await authenticate(identity);
  return identity;
}
export async function prove(identity, challenge = '0'.repeat(64), version = 'v2') {
  const path = await request(identity.url, `/api/registry/path/${identity.commitment}`);
  const topicHash = BigInt(`0x${digest(`annex/v2/topicHash:${identity.topic}`)}`) % prime;
  const circuit = version === 'v2' ? 'membership_v2' : 'membership';
  const input = {
    sk: identity.sk.toString(), roleCode: String(identity.roleCode), nodeId: String(identity.nodeId),
    leafIndex: String(path.leafIndex), pathElements: path.pathElements.map(value => `0x${value}`),
    pathIndexBits: path.pathIndexBits.map(String),
    ...(version === 'v2' ? { topicHash: topicHash.toString(), challenge: BigInt(`0x${challenge}`).toString() } : {}),
  };
  const { proof, publicSignals } = await groth16.fullProve(input,
    `${root}zk/build/${circuit}_js/${circuit}.wasm`, `${root}zk/keys/${circuit}_final.zkey`);
  assert.equal(publicSignals.length, version === 'v2' ? 5 : 2);
  assert.equal(hex(publicSignals[0]), path.rootHex);
  assert.equal(hex(publicSignals[1]), identity.commitment);
  return { proof, publicSignals, root: path.rootHex, topicHash: hex(topicHash) };
}
export async function authenticate(identity) {
  const challenge = await request(identity.url, '/api/zk/challenge', {
    method: 'POST', body: { commitment: identity.commitment, topic: identity.topic },
  });
  const proof = await prove(identity, challenge.challenge);
  const body = {
    root: proof.root, commitment: identity.commitment, topic: identity.topic,
    proof: proof.proof, publicSignals: proof.publicSignals, protocolVersion: 'v2',
    nullifierHex: hex(proof.publicSignals[2]), topicHashHex: proof.topicHash, challengeHex: challenge.challenge,
  };
  const session = await request(identity.url, '/api/zk/verify-membership', { method: 'POST', body });
  assert.equal(session.ok, true);
  assert.equal(typeof session.sessionToken, 'string');
  identity.token = session.sessionToken;
  identity.pseudonym = session.pseudonymId;
  proofHeaders.set(identity.token, Buffer.from(JSON.stringify({
    proof: proof.proof, root_hex: proof.root, commitment_hex: identity.commitment,
    protocolVersion: 'v2', publicSignals: proof.publicSignals, topic: identity.topic,
  })).toString('base64'));
  authenticatedIdentities.set(identity.token, identity);
  return identity;
}
export async function socket(identity) {
  const url = new URL('/ws', identity.url);
  url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
  url.searchParams.set('token', identity.token);
  const ws = new WebSocket(url);
  const frames = [];
  ws.addEventListener('message', event => { frames.push(JSON.parse(String(event.data))); });
  await new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('WebSocket connection timed out')), 20_000);
    ws.addEventListener('open', () => { clearTimeout(timeout); resolve(); }, { once: true });
    ws.addEventListener('error', () => { clearTimeout(timeout); reject(new Error('WebSocket connection failed')); }, { once: true });
  });
  return { ws, frames, send: frame => ws.send(JSON.stringify(frame)), wait: (predicate, label) => until(() => frames.find(predicate), label) };
}
export async function until(read, label, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const value = await read();
    if (value) return value;
    await new Promise(resolve => setTimeout(resolve, 200));
  }
  throw new Error(`Timed out waiting for ${label}`);
}
export function hashList(values) {
  const hash = createHash('sha256');
  for (const value of [...values].sort()) {
    const bytes = Buffer.from(value);
    const length = Buffer.alloc(8); length.writeBigUInt64BE(BigInt(bytes.length));
    hash.update(length); hash.update(bytes);
  }
  return hash.digest('hex');
}
export function handshake(policy) {
  return {
    anchor_snapshot: {
      principles_hash: hashList(policy.principles), prohibited_actions_hash: hashList(policy.prohibited_actions),
      timestamp: Math.floor(Date.now() / 1000), principles: policy.principles, prohibited_actions: policy.prohibited_actions,
    },
    capability_contract: { required_capabilities: [], offered_capabilities: policy.agent_required_capabilities, redacted_topics: [] },
  };
}
export function signingIdentity(file) {
  return signingIdentityFromSeed(readFileSync(file, 'utf8'));
}
export function signingIdentityFromSeed(value) {
  const seed = Buffer.from(value.trim(), 'hex');
  assert.equal(seed.length, 32, 'Server identity must be a persisted 32-byte Ed25519 seed');
  const privateKey = createPrivateKey({ key: Buffer.concat([Buffer.from('302e020100300506032b657004220420', 'hex'), seed]), format: 'der', type: 'pkcs8' });
  const publicKey = createPublicKey(privateKey).export({ format: 'der', type: 'spki' }).subarray(-32).toString('hex');
  return { publicKey, sign: message => sign(null, Buffer.from(message), privateKey).toString('hex') };
}
export async function createChannel(founder, channelId, type = 'Text', scope = 'Federated') {
  return request(founder.url, '/api/channels', { method: 'POST', token: founder.token, body: {
    channel_id: channelId, name: channelId, channel_type: type, topic: 'Real two-node acceptance',
    federation_scope: scope, vrp_topic_binding: null, required_capabilities_json: null, agent_min_alignment: null, retention_days: null,
  } });
}
export const joinChannel = (identity, channelId) => request(identity.url, `/api/channels/${channelId}/join`, { method: 'POST', token: identity.token, body: {} });
export const history = (identity, channelId) => request(identity.url, `/api/channels/${channelId}/messages?limit=100`, { token: identity.token });
