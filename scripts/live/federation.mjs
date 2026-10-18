// Usage: node scripts/live/federation.mjs <Windows session JSON> <Windows or Ubuntu peer JSON> [--rtx-contracts] [--channel-authority] [--encrypted-authority] [--authority-only]
// Only the administrator's peer trust anchors are provisioned directly. Agreements,
// identities, proofs, channel membership and traffic must cross the actual APIs.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, readdirSync, existsSync, realpathSync } from 'node:fs';
import { join, dirname, basename, relative, isAbsolute } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { execFileSync } from 'node:child_process';
import { hkdfSync, createDecipheriv } from 'node:crypto';
import { register, request, prove, hex, socket, handshake, signingIdentity, signingIdentityFromSeed, createChannel, joinChannel, history, until } from './protocol.mjs';
import { runRtxAcceptance } from './rtx.mjs';
import { contractV2, runRtxContractAcceptance } from './rtx-contract.mjs';
import { runChannelAuthorityAcceptance } from './channel-authority.mjs';
import { runEncryptedAuthorityAcceptance } from './encrypted-authority.mjs';

const [hostRecord, vmRecord, ...flags] = process.argv.slice(2);
assert(flags.every(flag => ['--rtx-contracts', '--channel-authority', '--encrypted-authority', '--authority-only'].includes(flag)), 'Unknown live federation lane flag');
const contractLane = flags.includes('--rtx-contracts');
const authorityLane = flags.includes('--channel-authority');
const encryptedAuthorityLane = flags.includes('--encrypted-authority');
const authorityOnly = flags.includes('--authority-only');
assert(!authorityOnly || ((authorityLane || encryptedAuthorityLane) && !contractLane), 'Authority-only requires an authority lane and cannot claim RTX coverage');
assert(hostRecord && vmRecord, 'Provide the two isolated server session records');
const host = JSON.parse(readFileSync(hostRecord, 'utf8').replace(/^\uFEFF/, ''));
const vm = JSON.parse(readFileSync(vmRecord, 'utf8').replace(/^\uFEFF/, ''));
const guestIsLinux = vm.data.startsWith('/');
const dataChannelRequired = Boolean(host.signal_base_url && vm.signal_base_url);
assert.equal(Boolean(host.signal_base_url), Boolean(vm.signal_base_url), 'Both peers must select the same acceptance transport');
const hostKey = signingIdentity(join(host.data, 'signing.key'));
// Read this disposable peer's operator-owned seed inside its VM. Never log or
// persist it in acceptance evidence; Windows UNC access uses a different user.
const vmKey = guestIsLinux ? signingIdentityFromSeed(execFileSync('wsl.exe', ['-d', 'Ubuntu', '-u', 'root', '--exec', 'cat', `${vm.data}/signing.key`], { encoding: 'utf8' })) : signingIdentity(join(vm.data, 'signing.key'));
const receipts = [];
const connections = [];
const guestWork = '/mnt/c/Users/monty/Documents/Codex/2026-10-12/i-need-you-to-take-annex/work';
const hostSessionName = basename(hostRecord, '.json').replace(/^session-/, '');
assert(/^[a-z0-9-]+$/.test(hostSessionName), 'Expected an isolated session record name');
function controlHost(action) {
  execFileSync(process.env.ANNEX_TEST_POWERSHELL ?? 'pwsh.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
    join(dirname(hostRecord), `${action}-session.ps1`), '-Name', hostSessionName], {
    // Windows descendants can retain inherited pipe handles after PowerShell
    // exits. The helper records readiness and PID on disk; no captured stdio is
    // needed and a background server must not hold execFileSync open forever.
    stdio: 'ignore', timeout: 60_000,
  });
}
function controlGuest(action) {
  if (!guestIsLinux) {
    const name = basename(vmRecord, '.json').replace(/^session-/, '');
    assert(/^[a-z0-9-]+$/.test(name));
    execFileSync(process.env.ANNEX_TEST_POWERSHELL ?? 'pwsh.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
      join(dirname(vmRecord), `${action}-session.ps1`), '-Name', name], { stdio: 'ignore', timeout: 60_000 });
    return;
  }
  execFileSync('wsl.exe', ['-d', 'Ubuntu', '-u', 'root', '--exec', 'bash', `${guestWork}/${action}-peer.sh`]);
}
function record(action, details = {}) {
  receipts.push({ at: new Date().toISOString(), action, ...details });
  console.log(`[federation-live] ${action}${action === 'FAILED' ? `: ${details.error}` : ''}`);
}
function guestRead(sql, params = []) {
  if (!guestIsLinux) {
    const db = new DatabaseSync(join(vm.data, 'annex.db'), { readOnly: true });
    try { return db.prepare(sql).all(...params); } finally { db.close(); }
  }
  const python = 'import sqlite3,json,sys,pathlib; c=sqlite3.connect(pathlib.Path(sys.argv[1]).resolve().as_uri()+"?mode=ro",uri=True); c.row_factory=sqlite3.Row; print(json.dumps([dict(r) for r in c.execute(sys.argv[2],json.loads(sys.argv[3]))]))';
  return JSON.parse(execFileSync('wsl.exe', ['-d', 'Ubuntu', '-u', 'root', '--exec', 'python3', '-c', python, `${vm.data}/annex.db`, sql, JSON.stringify(params)], { encoding: 'utf8' }));
}
function readRows(serverUrl, sql, params = []) {
  assert(/^SELECT\b/i.test(sql.trim()), 'Acceptance evidence reads must be SELECT only');
  if (serverUrl === vm.url) return guestRead(sql, params);
  assert.equal(serverUrl, host.url);
  const db = new DatabaseSync(join(host.data, 'annex.db'), { readOnly: true });
  try { return db.prepare(sql).all(...params); } finally { db.close(); }
}
function readUploadBytes(serverUrl, id, retainedPath = null) {
  assert(/^[0-9a-f-]{36}$/.test(id));
  const stored = retainedPath ?? readRows(serverUrl, 'SELECT storage_path FROM uploads WHERE upload_id=?', [id])[0]?.storage_path;
  assert.equal(typeof stored, 'string'); assert(!isAbsolute(stored) && !stored.split(/[\\/]/).includes('..'));
  if (serverUrl === vm.url && guestIsLinux) {
    const python = 'import pathlib,sys,base64; root=pathlib.Path(sys.argv[1]).resolve(); p=(root/sys.argv[2]).resolve(); assert p.is_relative_to(root); assert not p.exists() or p.stat().st_size<=52428828; print(base64.b64encode(p.read_bytes()).decode() if p.exists() else "ABSENT")';
    const bytes = execFileSync('wsl.exe', ['-d', 'Ubuntu', '-u', 'root', '--exec', 'python3', '-c', python, `${vm.data}/uploads`, stored], { encoding: 'utf8', maxBuffer: 72 * 1024 * 1024 }).trim();
    return bytes === 'ABSENT' ? null : Buffer.from(bytes, 'base64');
  }
  assert([host.url, vm.url].includes(serverUrl)); const root = realpathSync(join(serverUrl === host.url ? host.data : vm.data, 'uploads')); const path = join(root, stored);
  const confined = relative(root, path); assert(confined && !confined.startsWith('..') && !isAbsolute(confined));
  if (!existsSync(path)) return null;
  const resolved = relative(root, realpathSync(path)); assert(resolved && !resolved.startsWith('..') && !isAbsolute(resolved));
  return readFileSync(path);
}

// Read-only duplicate-delivery proof: open the already committed outbox body
// using its operator-owned test-node key. Never log the seed or plaintext
// envelope; only the unchanged signed object is returned to the live sender.
function readStoredOutboxEnvelope(serverUrl, rowId) {
  assert(Number.isSafeInteger(rowId) && rowId > 0);
  let row;
  let seed;
  if (serverUrl === vm.url) {
    row = guestRead('SELECT envelope_json FROM federation_outbox WHERE id = ?', [rowId])[0];
    seed = guestIsLinux ? execFileSync('wsl.exe', ['-d', 'Ubuntu', '-u', 'root', '--exec', 'cat', `${vm.data}/signing.key`], { encoding: 'utf8' }) : readFileSync(join(vm.data, 'signing.key'), 'utf8');
  } else {
    assert.equal(serverUrl, host.url);
    const db = new DatabaseSync(join(host.data, 'annex.db'), { readOnly: true });
    try { row = db.prepare('SELECT envelope_json FROM federation_outbox WHERE id = ?').get(rowId); }
    finally { db.close(); }
    seed = readFileSync(join(host.data, 'signing.key'), 'utf8');
  }
  assert(row?.envelope_json.startsWith('\x01ar1:'), 'RTX outbox must be encrypted at rest');
  const key = Buffer.from(hkdfSync('sha256', Buffer.from(seed.trim(), 'hex'),
    Buffer.from('annex-message-at-rest'), Buffer.from('annex-message-at-rest-v1'), 32));
  const blob = Buffer.from(row.envelope_json.slice(5), 'base64');
  assert(blob.length > 28);
  const decipher = createDecipheriv('chacha20-poly1305', key, blob.subarray(0, 12), { authTagLength: 16 });
  decipher.setAuthTag(blob.subarray(-16));
  try { return JSON.parse(Buffer.concat([decipher.update(blob.subarray(12, -16)), decipher.final()]).toString('utf8')); }
  finally { key.fill(0); }
}

async function federate(receiver, originUrl, key, policy, explicitContract = false) {
  const payload = handshake(policy);
  if (explicitContract) payload.capability_contract = contractV2(policy);
  const body = { base_url: originUrl, ...payload, signature: key.sign(`${originUrl}\n${JSON.stringify(payload)}`) };
  const accepted = await request(receiver, '/api/federation/handshake', { method: 'POST', body });
  assert.equal(accepted.alignment_status, 'Aligned');
  assert.notEqual(accepted.transfer_scope, 'NoTransfer');
  const altered = { ...body, signature: '00'.repeat(64) };
  await request(receiver, '/api/federation/handshake', { method: 'POST', body: altered, expected: 401 });
  record('Signed handshake accepted; forged signature refused', { receiver, alignment: accepted.alignment_status });
}
async function attest(receiver, identity, originUrl, key) {
  const proof = await prove(identity);
  const nullifierHex = hex(proof.publicSignals[2]);
  const topicHashHex = proof.topicHash;
  const signing = [identity.topic, identity.commitment, 'BRIDGE', 'v2', nullifierHex, topicHashHex].join('\n');
  const body = {
    originating_server: originUrl, topic: identity.topic, commitment: identity.commitment,
    proof: proof.proof, participant_type: 'BRIDGE', signature: key.sign(signing), protocolVersion: 'v2',
    publicSignals: proof.publicSignals, nullifierHex, topicHashHex,
  };
  const accepted = await request(receiver, '/api/federation/attest-membership', { method: 'POST', body });
  assert.equal(accepted.ok, true);
  assert.equal(accepted.pseudonymId, identity.pseudonym);
  const tampered = { ...body, publicSignals: [...proof.publicSignals.slice(0, 4), '1'] };
  await request(receiver, '/api/federation/attest-membership', { method: 'POST', body: tampered, expected: 400 });
  record('Pinned v2 proof accepted; changed challenge signal refused', { receiver, publicSignals: proof.publicSignals.length });
}
async function remoteJoin(receiver, identity, originUrl, key, channelId) {
  await request(receiver, `/api/federation/channels/${channelId}/join`, { method: 'POST', body: {
    originating_server: originUrl, pseudonym_id: identity.pseudonym,
    signature: key.sign(`${channelId}\n${identity.pseudonym}`),
  } });
}
async function main() {
  await request(host.url, '/health'); await request(vm.url, '/health');
  // The VM resolves the host's published URL itself; this is not a loopback alias.
  if (guestIsLinux) {
    const guestHealth = JSON.parse(execFileSync('wsl.exe', ['-d', 'Ubuntu', '--exec', 'curl', '--fail', '--silent', '--max-time', '10', `${host.url}/health`], { encoding: 'utf8' }));
    assert.equal(guestHealth.status, 'ok');
  }
  record('Actual server endpoints are reachable', { platforms: ['Windows', guestIsLinux ? 'Ubuntu' : 'Windows'], host: host.url, vm: vm.url, hostBinary: host.binary_sha256, vmBinary: vm.binary_sha256 });
  const db = new DatabaseSync(join(host.data, 'annex.db'));
  db.exec('PRAGMA busy_timeout=5000');
  assert.equal(db.prepare('SELECT COUNT(*) AS count FROM federation_agreements').get().count, 0, 'Fresh agreement state required');
  db.prepare("INSERT INTO instances(base_url, public_key, label) VALUES (?, ?, 'Acceptance peer')").run(vm.url, vmKey.publicKey);
  db.close();
  const python = "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); assert c.execute('SELECT COUNT(*) FROM federation_agreements').fetchone()[0]==0; c.execute('INSERT INTO instances(base_url, public_key, label) VALUES (?, ?, ?)', (sys.argv[2],sys.argv[3],'Windows acceptance peer')); c.commit()";
  if (guestIsLinux) execFileSync('wsl.exe', ['-d', 'Ubuntu', '-u', 'root', '--exec', 'python3', '-c', python, `${vm.data}/annex.db`, host.url, hostKey.publicKey]);
  else {
    const peerDb = new DatabaseSync(join(vm.data, 'annex.db'));
    try {
      assert.equal(peerDb.prepare('SELECT COUNT(*) AS count FROM federation_agreements').get().count, 0, 'Fresh peer agreement state required');
      peerDb.prepare("INSERT INTO instances(base_url, public_key, label) VALUES (?, ?, 'Windows acceptance peer')").run(host.url, hostKey.publicKey);
    } finally { peerDb.close(); }
  }
  record('Operator peer keys provisioned; no agreement or membership fixtures');
  const a = await register(host.url); const b = await register(vm.url);
  const policyA = await request(host.url, '/api/admin/policy', { token: a.token });
  const policyB = await request(vm.url, '/api/admin/policy', { token: b.token });
  await federate(vm.url, host.url, hostKey, policyA);
  await federate(host.url, vm.url, vmKey, policyB);
  if (!authorityOnly) {
  const bridgeA = await register(host.url, 4); const bridgeB = await register(vm.url, 4);
  await attest(vm.url, bridgeA, host.url, hostKey);
  await attest(host.url, bridgeB, vm.url, vmKey);
  const channelId = `federation-live-${Date.now()}`;
  await createChannel(a, channelId); await createChannel(b, channelId);
  await joinChannel(a, channelId); await joinChannel(b, channelId);
  await joinChannel(bridgeA, channelId); await joinChannel(bridgeB, channelId);
  await remoteJoin(vm.url, bridgeA, host.url, hostKey, channelId);
  await remoteJoin(host.url, bridgeB, vm.url, vmKey, channelId);
  const sa = await socket(bridgeA); const sb = await socket(bridgeB);
  connections.push(sa, sb);
  sa.send({ type: 'subscribe', channelId }); sb.send({ type: 'subscribe', channelId });
  const textA = `Host to peer ${Date.now()}`;
  sa.send({ type: 'message', channelId, content: textA, clientRequestId: 'federation-host-live' });
  const messageA = await sb.wait(frame => frame.type === 'message' && frame.content === textA, 'actual host message on peer WebSocket');
  assert.equal(messageA.senderPseudonym, bridgeA.pseudonym);
  const textB = `Peer to host ${Date.now()}`;
  sb.send({ type: 'message', channelId, content: textB, clientRequestId: 'federation-vm-live' });
  await sa.wait(frame => frame.type === 'message' && frame.content === textB, 'actual peer message on host WebSocket');
  record('Bidirectional signed message relay observed on independent WebSockets', { channelId });
  const edited = `${textA} edited`;
  sa.send({ type: 'edit_message', channelId, messageId: messageA.messageId, content: edited, clientRequestId: 'fed-edit' });
  await sb.wait(frame => frame.type === 'message_edited' && frame.content === edited, 'remote edit event');
  await until(async () => (await history(b, channelId)).some(message => message.message_id === messageA.messageId && message.content === edited), 'persisted remote edit');
  // Catch-up can deliver an edit before the live outbox worker reaches it.
  // Keep this message alive until that independent direct transport path has
  // committed its own completion; immediate deletion would tombstone the work.
  await until(() => {
    const db = new DatabaseSync(join(host.data, 'annex.db'), { readOnly: true });
    try { return db.prepare("SELECT COUNT(*) AS count FROM federation_outbox WHERE source_message_id=? AND message_id LIKE 'edit:%' AND status='delivered' AND removed_reason IS NULL").get(messageA.messageId).count > 0; }
    finally { db.close(); }
  }, 'direct edit outbox receipt before deleting its source message', 60_000);
  sa.send({ type: 'delete_message', channelId, messageId: messageA.messageId, clientRequestId: 'fed-delete' });
  await sb.wait(frame => frame.type === 'message_deleted' && frame.messageId === messageA.messageId, 'remote delete event');
  await until(async () => (await history(b, channelId)).some(message => message.message_id === messageA.messageId && message.deleted_at && message.content === ''), 'persisted remote tombstone');
  record('Remote edit and deletion persisted with no retained deleted content');

  // Partition the actual peer process, then reopen its original on-disk state.
  // The harness configurations use a five-second live freshness window and
  // one outbox attempt. Waiting beyond it forces authenticated recovery.
  const retained = `${channelId}-edit-after-partition`;
  const erased = `${channelId}-delete-after-partition`;
  sa.send({ type: 'message', channelId, content: retained, clientRequestId: 'pre-partition-edit' });
  const retainedMessage = await sb.wait(frame => frame.type === 'message' && frame.content === retained, 'pre-partition editable message');
  sa.send({ type: 'message', channelId, content: erased, clientRequestId: 'pre-partition-delete' });
  const erasedMessage = await sb.wait(frame => frame.type === 'message' && frame.content === erased, 'pre-partition deletable message');
  controlGuest('stop');
  await until(() => sb.ws.readyState === WebSocket.CLOSED, 'actual peer shutdown');
  const missed = `${channelId}-created-while-offline`;
  sa.send({ type: 'message', channelId, content: missed, clientRequestId: 'offline-create' });
  const missedMessage = await sa.wait(frame => frame.type === 'message' && frame.content === missed, 'local persist during partition');
  const newText = `${retained}-edited`;
  sa.send({ type: 'edit_message', channelId, messageId: retainedMessage.messageId, content: newText, clientRequestId: 'offline-edit' });
  await sa.wait(frame => frame.type === 'message_edited' && frame.content === newText, 'local edit during partition');
  sa.send({ type: 'delete_message', channelId, messageId: erasedMessage.messageId, clientRequestId: 'offline-delete' });
  await sa.wait(frame => frame.type === 'message_deleted' && frame.messageId === erasedMessage.messageId, 'local delete during partition');
  await new Promise(resolve => setTimeout(resolve, 7_000));
  await until(() => {
    const pendingDb = new DatabaseSync(join(host.data, 'annex.db'), { readOnly: true });
    try { return pendingDb.prepare("SELECT COUNT(*) AS count FROM federation_outbox WHERE source_message_id = ? AND status = 'failed'").get(missedMessage.messageId).count > 0; }
    finally { pendingDb.close(); }
  }, 'Missed message must exhaust real transport retry before peer restarts', 90_000);
  record('Real peer process stopped; creation/edit/delete persisted and live retry exhausted');
  controlGuest('restart');
  await until(async () => {
    const messages = await history(b, channelId);
    return messages.some(message => message.message_id === missedMessage.messageId && message.content === missed)
      && messages.some(message => message.message_id === retainedMessage.messageId && message.content === newText)
      && messages.some(message => message.message_id === erasedMessage.messageId && message.deleted_at && message.content === '');
  }, 'automatic persisted catch-up after real peer restart', 60_000);
  const receipt = guestRead('SELECT delivery_mode FROM federation_message_receipts WHERE message_id = ?', [missedMessage.messageId]);
  assert.equal(receipt[0]?.delivery_mode, 'catchup', 'Recovered creation must be admitted through the authenticated catch-up path');
  const oldHistory = guestRead('SELECT COUNT(*) AS count FROM message_edits WHERE message_id = ?', [erasedMessage.messageId]);
  assert.equal(oldHistory[0].count, 0, 'Deleted message revisions must be purged');
  record('Automatic restart catch-up recovered creation and edit, applied deletion, and purged deleted revisions', { deliveryMode: receipt[0].delivery_mode });
  }
  const rtxControls = {
    hostFounder: a, vmFounder: b, record,
    readStoredOutboxEnvelope,
    async setPeerOnline(serverUrl, online) {
      assert(serverUrl === vm.url || serverUrl === host.url);
      // Ensure-online is idempotent. Restarting an already recovered peer in a
      // finally block would itself disconnect the sessions being verified.
      if (online) {
        try {
          const response = await fetch(`${serverUrl}/health`, { signal: AbortSignal.timeout(2_000) });
          if (response.ok && (await response.json()).status === 'ok') return;
        } catch { /* The real peer is offline; reopen its captured disk state. */ }
      }
      if (serverUrl === vm.url) controlGuest(online ? 'restart' : 'stop');
      else { assert.equal(serverUrl, host.url); controlHost(online ? 'restart' : 'stop'); }
    },
    restartPeer(serverUrl) {
      if (serverUrl === vm.url) { controlGuest('stop'); controlGuest('restart'); }
      else { assert.equal(serverUrl, host.url); controlHost('restart'); }
    },
    async renegotiatePeers() {
      // Read current operator policy, construct explicit version 2 contracts,
      // and sign each fresh actual handshake with the corresponding persisted
      // peer key. No agreement rows or negotiation outcomes are injected.
      const currentA = await request(host.url, '/api/admin/policy', { token: a.token });
      const currentB = await request(vm.url, '/api/admin/policy', { token: b.token });
      await federate(vm.url, host.url, hostKey, currentA, true);
      await federate(host.url, vm.url, vmKey, currentB, true);
    },
    readStoredBundle(serverUrl, bundleId) {
      if (serverUrl === vm.url) {
        return guestRead('SELECT * FROM rtx_bundles WHERE bundle_id = ?', [bundleId])[0] ?? null;
      }
      assert.equal(serverUrl, host.url, 'Only the two actual acceptance databases may be read');
      const local = new DatabaseSync(join(host.data, 'annex.db'), { readOnly: true });
      try { return local.prepare('SELECT * FROM rtx_bundles WHERE bundle_id = ?').get(bundleId) ?? null; }
      finally { local.close(); }
    },
  };
  if (!authorityOnly) await runRtxAcceptance({ ...rtxControls, policyHost: policyA, policyVm: policyB });
  if (contractLane) {
    const contractResult = await runRtxContractAcceptance(rtxControls);
    assert.equal(contractResult.complete, true);
    record('Explicit version 2 RTX contract lane completed against actual peers', contractResult);
  }
  if (authorityLane) {
    const result = await runChannelAuthorityAcceptance({ ...rtxControls,
      publicKeys: [hostKey.publicKey, vmKey.publicKey], readRows, readUploadBytes });
    assert.equal(result.complete, true); record('Current ordered channel authority and ordinary attachment lane completed', result);
  }
  if (encryptedAuthorityLane) {
    const result = await runEncryptedAuthorityAcceptance({ ...rtxControls,
      publicKeys: [hostKey.publicKey, vmKey.publicKey], readRows, readUploadBytes });
    assert.equal(result.complete, true); record('Current encrypted authority and opaque cross-peer attachment lane completed', result);
  }
  if (dataChannelRequired) {
    const hostLogs = readdirSync(host.data).filter(name => name === 'stdout.log' || /^restart-.*\.stdout\.log$/.test(name))
      .map(name => readFileSync(join(host.data, name), 'utf8')).join('\n');
    const peerLogs = guestIsLinux ? execFileSync('wsl.exe', ['-d', 'Ubuntu', '-u', 'root', '--exec', 'cat', `${vm.data}/stdout.log`], { encoding: 'utf8' })
      : readdirSync(vm.data).filter(name => name === 'stdout.log' || /^restart-.*\.stdout\.log$/.test(name)).map(name => readFileSync(join(vm.data, name), 'utf8')).join('\n');
    const lines = `${hostLogs}\n${peerLogs}`.replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, '')
      .split('\n').filter(line => line.includes('federation operation acknowledged'));
    assert(lines.length > 0);
    assert(lines.every(line => line.includes('transport="webrtc"') && line.includes('receipt="verified"')),
      'Every application relay completion must be a verified data-channel receipt; HTTP fallback is forbidden');
    for (const endpoint of [...(!authorityOnly ? ['messages', 'edits', 'redactions', 'rtx', 'catch-up'] : []),
      ...(authorityLane || encryptedAuthorityLane ? ['human-membership', 'channel-authority/operations', 'channel-authority/catch-up', 'channel-authority/read-permit', 'attachments/chunk'] : [])]) {
      assert(lines.some(line => line.includes(`endpoint="/api/federation/${endpoint}"`)), `Actual data-channel completion for ${endpoint}`);
    }
    assert(lines.every(line => /bytes_sent=[1-9][0-9]*/.test(line) && /bytes_received=[1-9][0-9]*/.test(line)));
    record('Actual DTLS/SCTP application delivery and verified receipts cover all selected legacy and authority endpoints with no HTTP fallback', { verifiedOperationCount: lines.length });
  }
}
try { await main(); }
catch (error) { record('FAILED', { error: error.message }); process.exitCode = 1; }
finally {
  for (const connection of connections) connection.ws.close();
  writeFileSync(join(host.data, 'federation-live-evidence.json'), JSON.stringify({
    scope: [dataChannelRequired ? 'signed WebRTC data-channel federation' : 'signed HTTP federation', ...(!authorityOnly ? ['v2 membership attestation', 'offline message/edit/deletion recovery', 'signed RTX'] : []), ...(contractLane ? ['version 2 RTX domain/size/retention negotiation, real restart and expiry'] : []), ...(authorityLane ? ['certified HUMAN authority admission, ordinary attachments, signed replica restart catchup, membership/read/deletion lifecycle'] : []), ...(encryptedAuthorityLane ? ['independently verified member manifests/key wraps, bidirectional encrypted messages/attachments, real outage read refusal, device recovery/history admission, member removal/rotation and disk cleanup'] : [])],
    complete: process.exitCode !== 1, receipts,
  }, null, 2));
  // snarkjs worker threads otherwise keep a successful CLI alive indefinitely.
  process.exit(process.exitCode ?? 0);
}
