// A live companion for the two-Windows-session browser lane. Its stdin carries
// already genuine browser credentials in memory; stdout contains public receipts
// only. Trust anchors are operator configuration, never agreement/outbox fixtures.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { basename, dirname, join, resolve, sep } from 'node:path';
import { createInterface } from 'node:readline';
import { execFileSync } from 'node:child_process';
import { DatabaseSync } from 'node:sqlite';
import { register, request, prove, authenticate, hex, socket, handshake, signingIdentity, joinChannel, history, until } from './protocol.mjs';
import { contractV2 } from './rtx-contract.mjs';

const files = process.argv.slice(2).map(file => resolve(file));
assert.equal(files.length, 2, 'Pass exactly two isolated Windows session records');
const records = files.map(file => JSON.parse(readFileSync(file, 'utf8').replace(/^\uFEFF/, '')));
assert.notEqual(records[0].url, records[1].url);
for (let index = 0; index < records.length; index++) {
  const record = records[index]; const root = resolve(dirname(files[index]), 'sessions') + sep;
  assert(resolve(record.data).startsWith(root) && resolve(record.binary).startsWith(root), 'Confine process controls to captured acceptance sessions');
  assert.equal(new URL(record.url).hostname, '127.0.0.1', 'This adapter controls disposable local Windows peers only');
  assert.equal(createHash('sha256').update(readFileSync(record.binary)).digest('hex').toUpperCase(), record.binary_sha256.toUpperCase());
  assert(!record.signal_base_url, 'This UI lane uses configured HTTP; cross-host/DC qualification is separate');
}
const keys = records.map(record => signingIdentity(join(record.data, 'signing.key')));
const bridges = []; const connections = []; let headers; let started = false;
const channelId = `federation-ui-${Date.now()}`;
const channels = new Set([channelId]);
const stopped = new Set();
const emit = value => process.stdout.write(`${JSON.stringify(value)}\n`);
async function browserRequest(index, path, options = {}) {
  const response = await fetch(new URL(path, records[index].url), { method: options.method ?? 'GET',
    headers: { ...headers[index], 'content-type': 'application/json' },
    body: options.body === undefined ? undefined : JSON.stringify(options.body), signal: AbortSignal.timeout(30_000) });
  assert.equal(response.status, options.expected ?? 200, `Actual browser-authorized ${path}: HTTP${response.status}`);
  return response.json();
}
function controlSession(index, online) {
  assert(index === 0 || index === 1);
  const name = basename(files[index], '.json').replace(/^session-/, '');
  assert(/^[a-z0-9-]+$/.test(name));
  execFileSync(process.env.ANNEX_TEST_POWERSHELL ?? 'pwsh.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
    join(dirname(files[index]), `${online ? 'restart' : 'stop'}-session.ps1`), '-Name', name],
  { stdio: 'ignore', timeout: 65_000 });
  if (online) stopped.delete(index); else stopped.add(index);
}
async function reconnect(index) {
  connections[index]?.ws.close(); connections[index] = await socket(bridges[index]);
  for (const channel of channels) connections[index].send({ type: 'subscribe', channelId: channel });
}
async function attest(receiver, origin) {
  const identity = bridges[origin]; const proof = await prove(identity);
  const nullifierHex = hex(proof.publicSignals[2]);
  const input = [identity.topic, identity.commitment, 'BRIDGE', 'v2', nullifierHex, proof.topicHash].join('\n');
  const accepted = await request(records[receiver].url, '/api/federation/attest-membership', { method: 'POST', body: {
    originating_server: records[origin].url, topic: identity.topic, commitment: identity.commitment,
    proof: proof.proof, participant_type: 'BRIDGE', signature: keys[origin].sign(input), protocolVersion: 'v2',
    publicSignals: proof.publicSignals, nullifierHex, topicHashHex: proof.topicHash,
  } });
  assert.equal(accepted.pseudonymId, identity.pseudonym);
  await request(records[receiver].url, `/api/federation/channels/${channelId}/join`, { method: 'POST', body: {
    originating_server: records[origin].url, pseudonym_id: identity.pseudonym,
    signature: keys[origin].sign(`${channelId}\n${identity.pseudonym}`),
  } });
}
async function start(command) {
  assert(!started); started = true; headers = command.headers;
  assert(Array.isArray(headers) && headers.length === 2);
  for (let index = 0; index < 2; index++) {
    assert(headers[index].authorization && headers[index]['x-annex-zk-proof'], 'Actual browser registration/proof is required');
    const db = new DatabaseSync(join(records[index].data, 'annex.db')); db.exec('PRAGMA busy_timeout=5000');
    try {
      assert.equal(db.prepare('SELECT COUNT(*) AS n FROM federation_agreements').get().n, 0, 'Use fresh federation state');
      const remote = 1 - index;
      db.prepare('INSERT INTO instances(base_url,public_key,label) VALUES(?,?,?)')
        .run(records[remote].url, keys[remote].publicKey, `Actual UI peer ${remote + 1}`);
    } finally { db.close(); }
  }
  for (let origin = 0; origin < 2; origin++) {
    const policy = await browserRequest(origin, '/api/admin/policy');
    const payload = { ...handshake(policy), capability_contract: contractV2(policy) };
    const accepted = await request(records[1 - origin].url, '/api/federation/handshake', { method: 'POST', body: {
      base_url: records[origin].url, ...payload, signature: keys[origin].sign(`${records[origin].url}\n${JSON.stringify(payload)}`),
    } });
    assert.equal(accepted.alignment_status, 'Aligned'); assert.notEqual(accepted.transfer_scope, 'NoTransfer');
  }
  for (let index = 0; index < 2; index++) {
    await browserRequest(index, '/api/channels', { method: 'POST', body: {
      channel_id: channelId, name: channelId, channel_type: 'Text', federation_scope: 'Federated',
    } });
    await browserRequest(index, `/api/channels/${channelId}/join`, { method: 'POST', body: {} });
    bridges.push(await register(records[index].url, 4)); await joinChannel(bridges[index], channelId);
  }
  await attest(1, 0); await attest(0, 1); await reconnect(0); await reconnect(1);
  return { channelId, urls: records.map(record => record.url), binaryHashes: records.map(record => record.binary_sha256),
    authorityIdentities: keys.map(key => `annex-server:${createHash('sha256').update(Buffer.from(key.publicKey, 'hex')).digest('hex')}`),
    bridgePseudonyms: bridges.map(identity => identity.pseudonym) };
}
async function publish(command) {
  assert(command.source === 0 || command.source === 1); assert.equal(typeof command.text, 'string');
  const selectedChannel = command.channelId ?? channelId; assert(channels.has(selectedChannel));
  const source = command.source; const id = crypto.randomUUID();
  connections[source].send({ type: 'message', channelId: selectedChannel, content: command.text, clientRequestId: id });
  const accepted = await connections[source].wait(frame => frame.clientRequestId === id, 'committed origin acknowledgment');
  assert.equal(accepted.type, 'message'); assert.equal(accepted.content, command.text);
  if (command.delivery) {
    const delivered = await connections[1 - source].wait(frame => frame.type === 'message' && frame.messageId === accepted.messageId, 'independent peer WebSocket delivery');
    assert.equal(delivered.content, command.text);
  }
  return { messageId: accepted.messageId, channelId: selectedChannel };
}
async function createLegacyChannel(command) {
  assert(started && typeof command.channelId === 'string' && /^[a-z0-9-]{1,128}$/.test(command.channelId));
  assert(!channels.has(command.channelId));
  assert(Array.isArray(command.headers) && command.headers.length === 2);
  headers = command.headers;
  const channel = command.channelId;
  for (let index = 0; index < 2; index++) {
    await browserRequest(index, '/api/channels', { method: 'POST', body: {
      channel_id: channel, name: channel, channel_type: 'Text', federation_scope: 'Federated',
    } });
    await browserRequest(index, `/api/channels/${channel}/join`, { method: 'POST', body: {} });
    await joinChannel(bridges[index], channel);
  }
  // The two real bridge identities were already attested during start. Add
  // their actual signed remote memberships for this additional legacy channel.
  for (let origin = 0; origin < 2; origin++) await request(records[1-origin].url,
    `/api/federation/channels/${channel}/join`, { method: 'POST', body: {
      originating_server: records[origin].url, pseudonym_id: bridges[origin].pseudonym,
      signature: keys[origin].sign(`${channel}\n${bridges[origin].pseudonym}`),
    } });
  channels.add(channel);
  for (const connection of connections) connection.send({ type:'subscribe',channelId:channel });
  return { channelId:channel };
}
async function dispatch(command) {
  switch (command.action) {
    case 'start': return start(command);
    case 'publish': return publish(command);
    case 'create-legacy-channel': return createLegacyChannel(command);
    case 'offline':
      controlSession(command.peer, false);
      await until(async () => { try { await fetch(`${records[command.peer].url}/health`, { signal: AbortSignal.timeout(1_000) }); return false; } catch { return true; } }, 'actual peer process outage');
      return { peer: command.peer, online: false };
    case 'online':
      controlSession(command.peer, true); await reconnect(command.peer); return { peer: command.peer, online: true };
    case 'received': {
      // The independent protocol observer deliberately authenticates its
      // current genuine witness. Browser long-session renewal has a separate
      // sustained-session acceptance; this controller must not retain a stale
      // proof when a UI action registered another real member.
      await authenticate(bridges[command.peer]); await reconnect(command.peer);
      const selectedChannel = command.channelId ?? channelId; assert(channels.has(selectedChannel));
      const messages = await history(bridges[command.peer], selectedChannel);
      return { peer: command.peer, matching: messages.filter(message => message.message_id === command.messageId).map(message => ({
        messageId: message.message_id, content: message.content, sender: message.sender_pseudonym,
      })) };
    }
    case 'close':
      for (const index of [...stopped]) controlSession(index, true);
      for (const connection of connections) connection.ws.close();
      return { closed: true };
    default: throw new Error('Unknown live companion command');
  }
}
const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
for await (const line of lines) {
  let command;
  try {
    command = JSON.parse(line); const result = await dispatch(command);
    emit({ id: command.id, type: 'result', action: command.action, ...result });
    if (command.action === 'close') { lines.close(); process.exit(0); }
  } catch (error) {
    emit({ id: command?.id, type: 'error', action: command?.action, error: error.message });
  }
}
for (const index of [...stopped]) controlSession(index, true);
for (const connection of connections) connection.ws.close();
