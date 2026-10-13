import { expect, type Browser } from '@playwright/test';
import { createHash, createPublicKey, verify } from 'node:crypto';
import { createRequire } from 'node:module';
import { readFileSync } from 'node:fs';
import { completeStartup } from '../helpers';
import { baseURL, newActor, publicIdentities, read, type Actor } from './operator-helpers';
import type { Channel, Message } from '../../src/types';
import type { DeviceCertificate } from '../../src/lib/device-certificate';
import type { ManifestPage, SignedManifestEvent } from '../../src/lib/e2e-manifest';
import type { ChannelKeyDirectory, OwnChannelWraps } from '../../src/lib/e2e-v2-protocol';

export interface EncryptionActor extends Actor {
  sent: Array<{ type: string; channelId?: string; content?: string; clientRequestId?: string }>;
  received: Array<{ type: string; channelId?: string; content?: string; clientRequestId?: string; messageId?: string; senderPseudonym?: string }>;
  publishedKeys: number;
  uploadedWraps: number;
  deviceId: string | null;
  cryptoRequests: Array<{ at: number; path: string; method: string; status?: number }>;
  workerDiagnostics: string[];
}

export async function encryptionActor(browser: Browser, register = true): Promise<EncryptionActor> {
  const actor = Object.assign(await newActor(browser, false), { sent: [], received: [], publishedKeys: 0, uploadedWraps: 0, deviceId: null, cryptoRequests: [], workerDiagnostics: [] }) as EncryptionActor;
  actor.page.on('console', message => {
    if (/Content Security Policy|worker-src|script-src|Refused to create a worker/i.test(message.text())) actor.workerDiagnostics.push(message.text().slice(0, 1500));
  });
  actor.page.on('request', request => {
    const path = new URL(request.url()).pathname;
    if (/\/api\/keys|\/key-|\/registry\/path|\/zk\//.test(path)) actor.cryptoRequests.push({ at: Date.now(), path, method: request.method() });
  });
  actor.page.on('websocket', socket => socket.on('framesent', event => {
    let frame: EncryptionActor['sent'][number];
    try { frame = JSON.parse(String(event.payload)) as typeof frame; } catch { return; }
    if (frame.type === 'message') actor.sent.push(frame);
  }));
  actor.page.on('websocket', socket => socket.on('framereceived', event => {
    let frame: EncryptionActor['received'][number];
    try { frame = JSON.parse(String(event.payload)) as typeof frame; } catch { return; }
    if (frame.type === 'message' || frame.type === 'message_edited' || frame.type === 'message_deleted') {
      actor.received.push({ type: frame.type, channelId: frame.channelId, content: frame.content,
        clientRequestId: frame.clientRequestId, messageId: frame.messageId, senderPseudonym: frame.senderPseudonym });
    }
  }));
  actor.page.on('response', response => {
    const path = new URL(response.url()).pathname;
    if (/\/api\/keys|\/key-|\/registry\/path|\/zk\//.test(path)) actor.cryptoRequests.push({ at: Date.now(), path, method: response.request().method(), status: response.status() });
    if (!response.ok()) return;
    const route = new URL(response.url());
    if (route.origin !== new URL(baseURL).origin) return;
    if (response.request().method() === 'PUT' && route.pathname === '/api/keys/me') {
      actor.publishedKeys++;
      void response.json().then((body: { device_id: string }) => { actor.deviceId = body.device_id; })
        .catch(() => { actor.errors.push('Unable to inspect actual public device response'); });
    }
    if (response.request().method() === 'POST' && route.pathname.endsWith('/key-wraps')) actor.uploadedWraps++;
  });
  if (register) await completeStartup(actor.page);
  return actor;
}

export async function createText(actor: Actor, name: string): Promise<Channel> {
  const page = actor.page;
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await page.locator('.create-channel-btn').click();
  const dialog = page.getByRole('dialog');
  await dialog.getByPlaceholder('general').fill(name);
  await dialog.locator('select').selectOption('Text');
  await dialog.getByPlaceholder('What this channel is about').fill('Real encrypted browser acceptance');
  const pending = page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === '/api/channels');
  await dialog.getByRole('button', { name: 'Create', exact: true }).click();
  expect((await pending).status()).toBe(200);
  await expect(dialog).toBeHidden();
  const found = (await read<Channel[]>(actor, '/api/channels')).find(item => item.name === name);
  expect(found).toBeDefined();
  return found!;
}

export function bubble(actor: Actor, text: string) {
  return actor.page.locator('.message').filter({ has: actor.page.locator('.message-content', { hasText: text }) });
}

/** Observe actual browser wire ciphertext and match it to persistent history. */
export async function sendEncrypted(actor: EncryptionActor, channelId: string, plaintext: string): Promise<Message> {
  const start = actor.sent.length;
  await actor.page.getByPlaceholder('Type a message...').fill(plaintext);
  await actor.page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(bubble(actor, plaintext).locator('.reply-btn')).toBeVisible();
  await expect(bubble(actor, plaintext)).not.toHaveClass(/pending|failed/);
  await expect.poll(() => actor.sent.slice(start).filter(frame => frame.channelId === channelId).length).toBe(1);
  const frame = actor.sent.slice(start).find(item => item.channelId === channelId)!;
  expect(frame.content, 'The actual WebSocket message must contain an epoch and key-bound ciphertext').toMatch(/^e2e2:[1-9][0-9]*:[0-9a-f]{64}:[A-Za-z0-9+/=]+$/);
  expect(frame.content?.includes(plaintext), 'Plaintext must not be in the actual wire body').toBe(false);
  expect(Buffer.from(frame.content!.split(':')[3], 'base64').length).toBeGreaterThan(28);
  let persisted: Message | undefined;
  await expect.poll(async () => {
    persisted = (await read<Message[]>(actor, `/api/channels/${channelId}/messages?limit=100`)).find(item => item.content === frame.content);
    return Boolean(persisted);
  }, { message: 'Authenticated history must retain the exact browser ciphertext, not plaintext' }).toBe(true);
  return persisted!;
}

export async function ready(actor: EncryptionActor): Promise<void> {
  await expect(actor.page.locator('.channel-encryption-bar.encrypted.key-ready')).toBeVisible();
  await expect.poll(() => actor.deviceId).toMatch(/^[0-9a-f]{64}$/);
}

/** Compare the actual trust dialog with independently fetched signed genesis. */
export async function trust(actor: EncryptionActor, founder: EncryptionActor, channelId: string, cancelFirst = false) {
  const original = await read<ManifestPage>(founder, `/api/channels/${channelId}/key-manifest?after_seq=0&limit=200`);
  expect(original.genesis_hash).toMatch(/^[0-9a-f]{64}$/);
  await expect(actor.page.getByRole('button', { name: 'Verify encryption', exact: true })).toBeVisible();
  await actor.page.getByRole('button', { name: 'Verify encryption', exact: true }).click();
  const dialog = actor.page.getByRole('dialog', { name: 'Verify encryption', exact: true });
  await expect(dialog.locator('code')).toHaveText([original.authority_identity, original.genesis_hash!]);
  if (cancelFirst) {
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await expect(actor.page.getByRole('button', { name: 'Verify encryption', exact: true })).toBeVisible();
    await actor.page.getByRole('button', { name: 'Verify encryption', exact: true }).click();
  }
  await dialog.getByRole('button', { name: 'Trust this channel', exact: true }).click();
  await expect(dialog).toHaveCount(0);
  await expect(actor.page.locator('.channel-encryption-bar.key-unverified')).toHaveCount(0);
}

const hash = (value: string | Buffer) => createHash('sha256').update(value).digest('hex');
const canonical = (value: unknown): unknown => Array.isArray(value) ? value.map(canonical)
  : value !== null && typeof value === 'object'
    ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical((value as Record<string, unknown>)[key])])) : value;
const edPublic = (hex: string) => createPublicKey({ key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), Buffer.from(hex, 'hex')]), format: 'der', type: 'spki' });
const certificateHash = (c: DeviceCertificate) => hash(JSON.stringify(['annex-device-certificate-v1', c.server_identity, c.topic, c.pseudonym_id,
  c.device_id, c.x25519_pub_hex, c.ed25519_pub_hex, c.root_hex, c.commitment_hex, c.nullifier_hex, c.generation, c.previous_certificate_sha256]));
const verified = new Set<string>();

async function independentCertificate(c: DeviceCertificate) {
  const digest = certificateHash(c);
  const fullCertificate = hash(JSON.stringify(canonical(c)));
  if (verified.has(fullCertificate)) return;
  expect(c.device_id).toBe(hash(Buffer.from(c.x25519_pub_hex, 'hex')));
  const challenge = (BigInt(`0x${digest}`) % 21888242871839275222246405745257275088548364400416034343698204186575808495617n).toString(16).padStart(64, '0');
  expect(c.public_signals).toHaveLength(5);
  expect(c.public_signals.slice(0, 3)).toEqual([c.root_hex, c.commitment_hex, c.nullifier_hex]);
  const topic = (BigInt(`0x${hash(`annex/v2/topicHash:${c.topic}`)}`) % 21888242871839275222246405745257275088548364400416034343698204186575808495617n).toString(16).padStart(64, '0');
  expect(c.public_signals[3]).toBe(topic);
  expect(c.public_signals[4]).toBe(challenge);
  expect(c.pseudonym_id).toBe(hash(`${c.topic}:${c.nullifier_hex}`));
  expect(verify(null, Buffer.from(JSON.stringify(['annex-device-possession-v1', digest])), edPublic(c.ed25519_pub_hex), Buffer.from(c.signature_hex, 'hex'))).toBe(true);
  const { groth16 } = createRequire(new URL('../../../zk/package.json', import.meta.url))('snarkjs') as {
    groth16: { verify(key: unknown, signals: string[], proof: unknown): Promise<boolean> };
  };
  const vkey = JSON.parse(readFileSync(new URL('../../../zk/keys/membership_v2_vkey.json', import.meta.url), 'utf8'));
  expect(await groth16.verify(vkey, c.public_signals.map(value => BigInt(`0x${value}`).toString()), c.proof), 'Actual browser certificate must independently verify with the pinned membership circuit').toBe(true);
  verified.add(fullCertificate);
}

/** Independent Node/OpenSSL signature checks over actual browser-created history. */
export async function signedEvidence(actor: EncryptionActor, channelId: string) {
  const page = await read<ManifestPage>(actor, `/api/channels/${channelId}/key-manifest?after_seq=0&limit=200`);
  expect(page.next_after_seq).toBeNull();
  const certificates = new Map<string, DeviceCertificate>();
  let previous = '';
  for (let index = 0; index < page.events.length; index++) {
    const event: SignedManifestEvent = page.events[index];
    if ('certificate' in event.operation) {
      await independentCertificate(event.operation.certificate);
      certificates.set(certificateHash(event.operation.certificate), event.operation.certificate);
    }
    const certificate = certificates.get(event.signer_certificate_sha256);
    expect(certificate, 'Every actual signer has a verified device certificate').toBeDefined();
    expect(certificate!.pseudonym_id).toBe(event.signer_pseudonym_id);
    expect(certificate!.device_id).toBe(event.signer_device_id);
    expect(event.seq).toBe(index + 1); expect(event.previous_hash).toBe(previous);
    expect(event.authority_identity).toBe(page.authority_identity); expect(event.channel_id).toBe(channelId);
    const bytes = Buffer.from(JSON.stringify(canonical(['annex-channel-manifest-v1', 1, event.authority_identity, event.channel_id, event.seq,
      event.previous_hash, event.operation, event.signer_pseudonym_id, event.signer_device_id, event.signer_certificate_sha256])));
    expect(event.event_hash).toBe(hash(bytes));
    expect(verify(null, bytes, edPublic(certificate!.ed25519_pub_hex), Buffer.from(event.signature_hex, 'hex'))).toBe(true);
    previous = event.event_hash;
  }
  expect(page.head_hash).toBe(previous); expect(page.head_seq).toBe(page.events.length);
  expect(page.genesis_hash).toBe(page.events[0].event_hash);
  const directory = await read<ChannelKeyDirectory>(actor, `/api/channels/${channelId}/member-keys`);
  const own = await read<OwnChannelWraps>(actor, `/api/channels/${channelId}/key-wraps?device_id=${actor.deviceId}`);
  for (const wrap of own.wraps) {
    const authorization = wrap.authorization;
    expect(authorization, 'New channel wraps require a retained member signature').not.toBeNull();
    const receipt = authorization!;
    const certificate = certificates.get(receipt.sender_certificate_sha256)!;
    expect(certificate).toBeDefined();
    expect(receipt.manifest_hash).toBe(page.events[receipt.manifest_seq - 1].event_hash);
    expect(receipt.wrap_digests.some(value => value[1] === actor.deviceId && value[3] === hash(Buffer.from(wrap.wrapped_key_b64, 'base64')))).toBe(true);
    const bytes = Buffer.from(JSON.stringify(canonical(['annex-channel-key-authorization-v1', receipt.authority_identity, receipt.channel_id,
      receipt.sender_pseudonym_id, receipt.sender_device_id, receipt.sender_certificate_sha256, receipt.manifest_seq,
      receipt.manifest_hash, receipt.operation, receipt.expected_revision, receipt.epoch, receipt.key_id, receipt.wrap_digests])));
    expect(verify(null, bytes, edPublic(certificate.ed25519_pub_hex), Buffer.from(receipt.signature_hex, 'hex'))).toBe(true);
  }
  return { page, directory, own, publicReceipt: { authority: page.authority_identity, genesis: page.genesis_hash, head: page.head_hash,
    signedEvents: page.head_seq, independentlyVerifiedCertificates: certificates.size, signedWraps: own.wraps.length,
    currentEpoch: directory.current_epoch, currentKeyId: directory.current_key_id } };
}

/** Use the shipped encrypted transfer UI. No keys or pairing codes leave memory. */
export async function linkDevice(source: EncryptionActor, target: EncryptionActor): Promise<void> {
  await source.page.getByRole('button', { name: 'Link', exact: true }).click();
  await source.page.getByRole('button', { name: /Link a New Device/ }).click();
  const transfer = await source.page.getByLabel('Transfer code', { exact: true }).inputValue();
  const code = (await source.page.locator('.pairing-code-value').innerText()).trim();
  await target.page.goto('/');
  await target.page.getByRole('button', { name: 'Link from Another Device', exact: true }).click();
  await target.page.getByRole('button', { name: /Receive from Another Device/ }).click();
  await target.page.getByPlaceholder('Paste the encoded data here...').fill(transfer);
  await target.page.getByPlaceholder('000000').fill(code);
  await target.page.getByRole('button', { name: 'Import Identity', exact: true }).click();
  await expect.poll(async () => (await publicIdentities(target)).length).toBe(1);
  const continued = target.page.getByRole('dialog').getByRole('button', { name: 'Continue', exact: true });
  if (await continued.isVisible().catch(() => false)) await continued.click();
  await source.page.getByRole('dialog').getByRole('button', { name: 'Done', exact: true }).click();
  await completeStartup(target.page);
}
