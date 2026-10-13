// @vitest-environment node
// The IndexedDB emulator uses Node's structuredClone; keep its typed arrays in
// the same realm as the keyring instead of mixing Node and jsdom globals.
import 'fake-indexeddb/auto';
import { deleteDB, openDB } from 'idb';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { channelKeyId } from './e2e-v2';
import { closeE2eKeyringV2, getDeviceSigningMaterial, indexedDbKeyringV2, pinEncryptionServer } from './e2e-keyring-v2';
import type { RetainedChannelKey } from './e2e-v2-protocol';

const server = `annex-server:${'a'.repeat(64)}`;
const otherServer = `annex-server:${'b'.repeat(64)}`;
const authority = 'https://annex.example';
const secret = new Uint8Array(32).fill(19);
function key(epoch: number, byte: number, purpose: 'current' | 'legacy' = 'current'): RetainedChannelKey {
  const cek = new Uint8Array(32).fill(byte);
  return { authorityServer: authority, epoch, keyId: channelKeyId(cek), cek, purpose };
}
async function legacyDatabase() {
  return openDB('annex-e2e', 1, {
    upgrade(db) { db.createObjectStore('device_secrets'); db.createObjectStore('channel_keys'); },
  });
}
beforeEach(async () => { await closeE2eKeyringV2(); await deleteDB('annex-e2e'); });
afterEach(async () => { await closeE2eKeyringV2(); await deleteDB('annex-e2e'); });

describe('durable device/epoch keyring migration', () => {
  it('persists one independent Ed25519 secret for each immutable encryption device across reloads', async () => {
    const device = await indexedDbKeyringV2().getOrCreateDevice(server, 'alice');
    const signatures = await Promise.all(Array.from({ length: 5 }, () => getDeviceSigningMaterial(server, 'alice', device.deviceId)));
    for (const signature of signatures) expect(signature.secret).toEqual(signatures[0].secret);
    expect(signatures[0].secret).not.toEqual(device.secret);
    await closeE2eKeyringV2();
    expect(await getDeviceSigningMaterial(server, 'alice', device.deviceId)).toEqual(signatures[0]);
    const next = await indexedDbKeyringV2().replaceDevice(server, 'alice', device.deviceId);
    expect((await getDeviceSigningMaterial(server, 'alice', next.deviceId)).secret).not.toEqual(signatures[0].secret);
    expect(await getDeviceSigningMaterial(server, 'alice', device.deviceId)).toEqual(signatures[0]);
  });

  it('keeps the first server identity pin across restart and refuses silent substitution', async () => {
    await pinEncryptionServer('https://one.example', server);
    await closeE2eKeyringV2();
    await expect(pinEncryptionServer('https://one.example', otherServer)).rejects.toThrow('identity changed');
    await pinEncryptionServer('https://two.example', otherServer);
    await pinEncryptionServer('https://one.example', server);
  });
  it('preserves the original device and legacy channel record byte-for-byte during version upgrade', async () => {
    const old = await legacyDatabase();
    await old.put('device_secrets', secret, 'alice');
    const oldKey = { epoch: 7, cek: new Uint8Array(32).fill(23) };
    await old.put('channel_keys', oldKey, 'alice:channel');
    old.close();

    const ring = indexedDbKeyringV2();
    const device = await ring.getOrCreateDevice(server, 'alice');
    expect(device.secret).toEqual(secret);
    expect(await ring.getUnscopedLegacyKey('alice', 'channel')).toEqual(oldKey);
    await closeE2eKeyringV2();
    const raw = await openDB('annex-e2e', 4);
    try {
      expect(await raw.get('device_secrets', 'alice')).toEqual(secret);
      expect(await raw.get('channel_keys', 'alice:channel')).toEqual(oldKey);
    } finally { raw.close(); }
  });

  it('scopes key histories by stable server, identity and arbitrary channel ID', async () => {
    const ring = indexedDbKeyringV2();
    await ring.putKey(server, 'alice', 'channel', key(1, 1));
    await ring.putKey(otherServer, 'alice', 'channel', key(1, 2));
    await ring.putKey(server, 'bob', 'channel', key(1, 3));
    await ring.putKey(server, 'alice:channel', 'other', key(1, 4));
    expect(await ring.getKeys(server, 'alice', 'channel')).toEqual([key(1, 1)]);
    expect(await ring.getKeys(otherServer, 'alice', 'channel')).toEqual([key(1, 2)]);
    expect(await ring.getKeys(server, 'alice', 'channel:other')).toEqual([]);
  });

  it('retains every historical epoch across closing and reopening the database', async () => {
    const ring = indexedDbKeyringV2();
    await ring.putKey(server, 'alice', 'channel', key(1, 1));
    await ring.putKey(server, 'alice', 'channel', key(2, 2));
    await closeE2eKeyringV2();
    expect(await indexedDbKeyringV2().getKeys(server, 'alice', 'channel')).toEqual([key(1, 1), key(2, 2)]);
  });

  it('refuses to overwrite an established epoch or change its authority', async () => {
    const ring = indexedDbKeyringV2();
    await ring.putKey(server, 'alice', 'channel', key(1, 1));
    await expect(ring.putKey(server, 'alice', 'channel', key(1, 2))).rejects.toThrow('immutable');
    await expect(ring.putKey(server, 'alice', 'channel', { ...key(1, 1), authorityServer: 'https://other.example' })).rejects.toThrow('immutable');
    expect(await ring.getKeys(server, 'alice', 'channel')).toEqual([key(1, 1)]);
  });

  it('retains different legacy candidates and never downgrades an authenticated candidate', async () => {
    const ring = indexedDbKeyringV2();
    await ring.putKey(server, 'alice', 'channel', { ...key(1, 1, 'legacy'), verifiedLegacy: true });
    await ring.putKey(server, 'alice', 'channel', { ...key(1, 1, 'legacy'), verifiedLegacy: false });
    await ring.putKey(server, 'alice', 'channel', key(1, 2, 'legacy'));
    const keys = await ring.getKeys(server, 'alice', 'channel');
    expect(keys).toHaveLength(2);
    expect(keys.find(value => value.keyId === key(1, 1).keyId)?.verifiedLegacy).toBe(true);
  });

  it('concurrent initializations converge on one persistent device', async () => {
    const ring = indexedDbKeyringV2();
    const devices = await Promise.all(Array.from({ length: 5 }, () => ring.getOrCreateDevice(server, 'alice')));
    expect(new Set(devices.map(device => device.deviceId)).size).toBe(1);
    await closeE2eKeyringV2();
    expect(await indexedDbKeyringV2().getOrCreateDevice(server, 'alice')).toEqual(devices[0]);
  });

  it('fails on corrupt legacy material instead of silently generating a replacement', async () => {
    const old = await legacyDatabase();
    await old.put('device_secrets', new Uint8Array(31), 'alice');
    old.close();
    await expect(indexedDbKeyringV2().getOrCreateDevice(server, 'alice')).rejects.toThrow('invalid');
    await closeE2eKeyringV2();
    const raw = await openDB('annex-e2e', 4);
    try {
      expect(await raw.count('device_keys_v2')).toBe(0);
      expect(await raw.get('device_secrets', 'alice')).toEqual(new Uint8Array(31));
    } finally { raw.close(); }
  });

  it('explicit replacement preserves the retired secret and prevents stale-window replacement', async () => {
    const ring = indexedDbKeyringV2();
    const first = await ring.getOrCreateDevice(server, 'alice');
    const second = await ring.replaceDevice(server, 'alice', first.deviceId);
    expect(second.deviceId).not.toBe(first.deviceId);
    expect(await ring.getOrCreateDevice(server, 'alice')).toEqual(second);
    expect(await ring.getLegacyDeviceSecrets(server, 'alice')).toEqual(expect.arrayContaining([first.secret, second.secret]));
    await expect(ring.replaceDevice(server, 'alice', first.deviceId)).rejects.toThrow('already replaced');
    expect(await ring.getOrCreateDevice(server, 'alice')).toEqual(second);
  });
});
