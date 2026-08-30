#!/usr/bin/env node
//
// smoke-federation-relay.mjs — LIVE cross-server federation proof.
//
// Drives the real inbound federation path of a running annex-server ("Server
// B") over HTTP, playing the role of a remote peer ("Server A") whose Ed25519
// keypair we control. We seed B's DB exactly the way a completed
// handshake+attestation would (instance + active agreement + attested
// federated identity + federated channel + membership) — those steps require
// B to reach A's VRP-root endpoint, which the SSRF guard correctly refuses for
// a loopback peer, so we seed their *result* — then sign a real
// FederatedMessageEnvelope with A's private key and POST it to
// B:/api/federation/messages. B verifies the Ed25519 signature against the
// registered pubkey, checks the agreement/identity/channel/membership, and
// persists + broadcasts the message under the locally-attested pseudonym.
//
// Proof points:
//   1. A correctly-signed envelope is accepted (200 {"status":"received"}).
//   2. The message is persisted on B under the attested local pseudonym.
//   3. A re-POST is idempotent (no duplicate row).
//   4. A TAMPERED signature is REJECTED — proving the crypto is enforced, not
//      a rubber stamp.
//
// Usage: node scripts/smoke-federation-relay.mjs --db <serverB.db> --url <serverB-url>

import { generateKeyPairSync, sign as edSign } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { createDecipheriv, hkdfSync, randomBytes } from 'node:crypto';
import { readFileSync } from 'node:fs';
import path from 'node:path';

function parseArgs(argv) {
  const out = { db: null, url: null };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--db') out.db = argv[++i];
    else if (argv[i] === '--url') out.url = argv[++i];
  }
  return out;
}
const log = (m) => console.log(`[fed-relay] ${m}`);
function fail(m, e) {
  console.error(`[fed-relay] FAIL: ${m}`);
  if (e) console.error(e.stack ?? e.message ?? String(e));
  process.exit(1);
}

function sqlite(db, sql) {
  // -batch -bail: fail hard on any SQL error.
  try {
    return execFileSync('sqlite3', ['-batch', '-bail', db, sql], { encoding: 'utf8' });
  } catch (e) {
    if (e.code === 'ENOENT') {
      fail('the `sqlite3` command is required by this smoke and is not installed');
    }
    throw e;
  }
}

// ── Encryption at rest ────────────────────────────────────────────────────
//
// Message bodies for non-E2E channels are stored as
// `"\x01ar1:" + base64(nonce(12) || ChaCha20Poly1305(content))` — see
// `crates/annex-server/src/at_rest.rs`. It is transparent through the
// server's own read path, but this smoke reads the SQLite file directly, so
// it has to unwrap the value itself. It did not, and compared the ciphertext
// against the plaintext it had relayed: the whole smoke failed with
// "persisted content mismatch: ar1:…" on a server that had done everything
// right. Decrypting rather than skipping the check makes the assertion
// stronger than the string compare it replaces — it now also proves the
// at-rest layer round-trips content that arrived over federation.
const AT_REST_MARKER = Buffer.from([0x01, 0x61, 0x72, 0x31, 0x3a]); // \x01 a r 1 :

function atRestKey(dbPath) {
  // The server derives this from its Ed25519 signing key, which
  // `resolve_signing_key` keeps beside the database as hex.
  const keyFile = path.join(path.dirname(path.resolve(dbPath)), 'signing.key');
  const signingKey = Buffer.from(readFileSync(keyFile, 'utf8').trim(), 'hex');
  if (signingKey.length !== 32) fail(`signing key at ${keyFile} is not 32 bytes`);
  return Buffer.from(
    hkdfSync('sha256', signingKey, Buffer.from('annex-message-at-rest'), Buffer.from('annex-message-at-rest-v1'), 32),
  );
}

function decryptAtRest(stored, key) {
  // Legacy-tolerant in the same way the server is: anything without the
  // marker is returned unchanged, so a plaintext row still reads correctly.
  if (!stored.subarray(0, AT_REST_MARKER.length).equals(AT_REST_MARKER)) {
    return stored.toString('utf8');
  }
  const blob = Buffer.from(stored.subarray(AT_REST_MARKER.length).toString('utf8'), 'base64');
  const nonce = blob.subarray(0, 12);
  const tag = blob.subarray(blob.length - 16);
  const ct = blob.subarray(12, blob.length - 16);
  const d = createDecipheriv('chacha20-poly1305', key, nonce, { authTagLength: 16 });
  d.setAuthTag(tag);
  return Buffer.concat([d.update(ct), d.final()]).toString('utf8');
}

// Raw 32-byte Ed25519 public key from a Node KeyObject (DER SPKI suffix).
function rawPubKeyHex(publicKey) {
  const der = publicKey.export({ type: 'spki', format: 'der' });
  return Buffer.from(der.subarray(der.length - 32)).toString('hex');
}

function v1SigningInput(e) {
  return [
    e.message_id,
    e.channel_id,
    e.content,
    e.sender_pseudonym,
    e.originating_server,
    e.attestation_ref,
    e.created_at,
  ].join('\n');
}

async function postEnvelope(url, envelope) {
  const res = await fetch(`${url}/api/federation/messages`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(envelope),
  });
  const text = await res.text();
  return { status: res.status, text };
}

async function main() {
  const { db, url } = parseArgs(process.argv.slice(2));
  if (!db || !url) fail('usage: --db <serverB.db> --url <serverB-url>');
  log(`server B db:  ${db}`);
  log(`server B url: ${url}`);

  // ── Server A's identity (we hold the private key) ──────────────────
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const pubHex = rawPubKeyHex(publicKey);
  const remoteOrigin = 'http://127.0.0.1:59999'; // loopback → B never calls out
  const topic = 'annex:server:v1';
  const commitmentHex = randomBytes(32).toString('hex');
  const attestationRef = `${topic}:${commitmentHex}`;
  const localPseudonym = `federated-alice-${Date.now().toString(36)}`;
  const channelId = `fed-live-${Date.now().toString(36)}`;
  log(`A pubkey: ${pubHex.slice(0, 16)}…  channel: ${channelId}`);

  // ── Seed B with the post-handshake/attestation state ───────────────
  // server_id 1 is the default server seeded at startup.
  const esc = (s) => s.replace(/'/g, "''");
  sqlite(
    db,
    [
      `INSERT INTO instances (base_url, public_key, label, status)
         VALUES ('${esc(remoteOrigin)}', '${esc(pubHex)}', 'Server A', 'ACTIVE');`,
      `INSERT INTO federation_agreements
         (local_server_id, remote_instance_id, alignment_status, transfer_scope, agreement_json, active)
         VALUES (1, (SELECT id FROM instances WHERE base_url='${esc(remoteOrigin)}'),
                 'ALIGNED', 'FULL_KNOWLEDGE_BUNDLE', '{}', 1);`,
      `INSERT INTO federated_identities
         (server_id, remote_instance_id, commitment_hex, pseudonym_id, vrp_topic, attested_at)
         VALUES (1, (SELECT id FROM instances WHERE base_url='${esc(remoteOrigin)}'),
                 '${esc(commitmentHex)}', '${esc(localPseudonym)}', '${esc(topic)}', datetime('now'));`,
      `INSERT INTO platform_identities (server_id, pseudonym_id, participant_type, active)
         VALUES (1, '${esc(localPseudonym)}', 'HUMAN', 1);`,
      `INSERT INTO channels (server_id, channel_id, name, channel_type, federation_scope, created_at)
         VALUES (1, '${esc(channelId)}', 'Fed Live', '"Text"', '"Federated"', datetime('now'));`,
      `INSERT INTO channel_members (server_id, channel_id, pseudonym_id, role, joined_at)
         VALUES (1, '${esc(channelId)}', '${esc(localPseudonym)}', 'MEMBER', datetime('now'));`,
    ].join('\n'),
  );
  log('seeded B: instance + active agreement + attested identity + federated channel + membership');

  // ── 1. Relay a correctly-signed message A → B ──────────────────────
  const messageId = `msg-live-${Date.now().toString(36)}`;
  const content = `Hello from Server A — ${new Date().toISOString()}`;
  const envelope = {
    envelope_version: null, // v1
    message_id: messageId,
    channel_id: channelId,
    content,
    sender_pseudonym: 'alice-on-server-a',
    originating_server: remoteOrigin,
    attestation_ref: attestationRef,
    created_at: new Date().toISOString(),
    signature: '',
  };
  envelope.signature = Buffer.from(
    edSign(null, Buffer.from(v1SigningInput(envelope)), privateKey),
  ).toString('hex');

  log('POST /api/federation/messages (signed)');
  const r1 = await postEnvelope(url, envelope);
  if (r1.status !== 200) fail(`relay rejected: ${r1.status} ${r1.text}`);
  log(`accepted: ${r1.status} ${r1.text.trim()}`);

  // ── 2. Verify persistence under the attested local pseudonym ───────
  // hex(), not the raw column: the stored value is binary-prefixed and
  // base64, and round-tripping it through the sqlite3 CLI as text is a way to
  // lose bytes for no reason.
  const row = sqlite(
    db,
    `SELECT hex(content) || '|' || sender_pseudonym FROM messages WHERE message_id='${esc(messageId)}';`,
  ).trim();
  if (!row) fail('message was not persisted on B');
  const [contentHex, gotSender] = row.split('|');
  let gotContent;
  try {
    gotContent = decryptAtRest(Buffer.from(contentHex, 'hex'), atRestKey(db));
  } catch (e) {
    fail('stored content did not decrypt with the server at-rest key', e);
  }
  if (gotContent !== content) fail(`persisted content mismatch: ${gotContent}`);
  if (gotSender !== localPseudonym) {
    fail(`expected message mapped to attested local pseudonym ${localPseudonym}, got ${gotSender}`);
  }
  log(`persisted on B: content matches, sender mapped to local pseudonym ${gotSender}`);

  // ── 3. Idempotent re-delivery (no duplicate) ───────────────────────
  const r2 = await postEnvelope(url, envelope);
  if (r2.status !== 200) fail(`idempotent re-POST should be accepted, got ${r2.status} ${r2.text}`);
  const count = sqlite(
    db,
    `SELECT COUNT(*) FROM messages WHERE message_id='${esc(messageId)}';`,
  ).trim();
  if (count !== '1') fail(`expected exactly 1 persisted row after re-POST, got ${count}`);
  log('idempotent: re-POST accepted, still exactly 1 row');

  // ── 4. Tampered signature MUST be rejected ─────────────────────────
  const tampered = { ...envelope, message_id: `${messageId}-tamper`, content: `${content} (tampered)` };
  // keep the OLD signature → no longer matches the body
  const r3 = await postEnvelope(url, tampered);
  if (r3.status === 200) {
    fail('SECURITY: B accepted a message whose signature does not match its body!');
  }
  log(`tampered envelope correctly rejected: ${r3.status}`);

  log('OK — LIVE federation relay proven (signed accept + persist + idempotent + tamper-reject)');
}

main()
  .then(() => process.exit(0))
  .catch((err) => fail('unexpected error', err));
