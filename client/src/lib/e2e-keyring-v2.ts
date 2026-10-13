/** Non-destructive upgrade of the original IndexedDB encryption store. */
import { openDB, type IDBPDatabase } from 'idb';
import { E2eError, generateDeviceSecret, publicKeyFromSecret } from './e2e';
import { channelKeyId, deviceIdForPublicKey, requireSafeInteger } from './e2e-v2';
import type { E2eKeyringV2, LocalEncryptionDevice, RetainedChannelKey } from './e2e-v2-protocol';
import { ed25519 } from '@noble/curves/ed25519.js';
import { certificateDigest, parseDeviceCertificate, type DeviceCertificate } from './device-certificate';
import { toHex } from './e2e';
import { checkManifestPin, type ManifestPin, type VerifiedManifest } from './e2e-manifest';

const LEGACY_DEVICES = 'device_secrets';
const LEGACY_CHANNELS = 'channel_keys';
const DEVICES = 'device_keys_v2';
const ACTIVE_DEVICES = 'active_devices_v2';
const KEYS = 'channel_keyring_v2';
const SIGNING = 'device_signing_keys_v1';
const CERTIFICATES = 'device_certificates_v1';
const SERVER_PINS = 'encryption_server_pins_v1';
const MANIFEST_PINS = 'channel_manifest_pins_v1';
let database: Promise<IDBPDatabase> | null = null;

function getDatabase(): Promise<IDBPDatabase> {
  if (!database) {
    database = openDB('annex-e2e', 4, {
      upgrade(db) {
        for (const store of [LEGACY_DEVICES, LEGACY_CHANNELS, DEVICES, ACTIVE_DEVICES, KEYS, SIGNING, CERTIFICATES, SERVER_PINS, MANIFEST_PINS]) {
          if (!db.objectStoreNames.contains(store)) db.createObjectStore(store);
        }
      },
      blocking(_current, _next, event) {
        (event.target as IDBDatabase).close();
        database = null;
      },
      terminated() { database = null; },
    }).catch(error => { database = null; throw error; });
  }
  return database;
}

export async function closeE2eKeyringV2(): Promise<void> {
  const pending = database;
  database = null;
  if (pending) (await pending).close();
}

export async function getChannelManifestPin(authorityIdentity: string, pseudonym: string, channelId: string): Promise<ManifestPin | null> {
  const db = await getDatabase();
  return (await db.get(MANIFEST_PINS, [...scope(authorityIdentity, pseudonym), channelId]) as ManifestPin | undefined) ?? null;
}
export async function assertPinnedEncryptionServer(baseUrl: string, serverIdentity: string): Promise<void> {
  const db = await getDatabase();
  const previous = await db.get(SERVER_PINS, baseUrl) as string | undefined;
  if (previous !== undefined && previous !== serverIdentity) throw new E2eError('The encryption server identity changed');
}

/** Atomically compare the latest durable pin, including updates from another tab. */
export async function retainChannelManifestPin(
  pseudonym: string, history: Map<number, VerifiedManifest>, final: VerifiedManifest, allowFirstTrust = false,
): Promise<void> {
  if (!final.head) throw new E2eError('An empty manifest cannot be trusted');
  const db = await getDatabase();
  const tx = db.transaction(MANIFEST_PINS, 'readwrite');
  const key = [...scope(final.authorityIdentity, pseudonym), final.channelId];
  const previous = (await tx.store.get(key) as ManifestPin | undefined) ?? null;
  try {
    if (!previous && !allowFirstTrust) throw new E2eError('Verify this channel before trusting its encryption');
    checkManifestPin(previous, history, final);
    await tx.store.put(final.head, key);
    await tx.done;
  } catch (error) {
    try { tx.abort(); } catch { /* A failed transaction may already have closed. */ }
    await tx.done.catch(() => {});
    throw error;
  }
}

function scope(serverIdentity: string, pseudonym: string): [string, string] {
  if (!/^annex-server:[0-9a-f]{64}$/.test(serverIdentity) || !pseudonym) throw new E2eError('Invalid encryption storage context');
  return [serverIdentity, pseudonym];
}

function secretBytes(value: unknown): Uint8Array {
  if (!(value instanceof Uint8Array) || value.length !== 32) throw new E2eError('Stored device secret is invalid');
  return value.slice();
}

function deviceFromSecret(value: unknown): LocalEncryptionDevice {
  const secret = secretBytes(value);
  const publicKey = publicKeyFromSecret(secret);
  return { secret, publicKey, deviceId: deviceIdForPublicKey(publicKey) };
}

function retainedKey(value: unknown): RetainedChannelKey {
  if (!value || typeof value !== 'object') throw new E2eError('Stored channel key is invalid');
  const key = value as RetainedChannelKey;
  if (typeof key.authorityServer !== 'string' || !key.authorityServer
    || (key.purpose !== 'current' && key.purpose !== 'legacy')) throw new E2eError('Stored channel key context is invalid');
  requireSafeInteger(key.epoch, 1);
  const cek = secretBytes(key.cek);
  if (channelKeyId(cek) !== key.keyId) throw new E2eError('Stored channel key fingerprint is invalid');
  return { ...key, cek };
}

/**
 * Array keys separate arbitrary channel IDs without delimiter collisions.
 * Legacy stores remain byte-for-byte intact, including old device material
 * that can still open wraps created before another device replaced its key.
 */
export function indexedDbKeyringV2(): E2eKeyringV2 {
  return {
    async getOrCreateDevice(serverIdentity, pseudonym) {
      const identity = scope(serverIdentity, pseudonym);
      const db = await getDatabase();
      const tx = db.transaction([DEVICES, ACTIVE_DEVICES, LEGACY_DEVICES], 'readwrite');
      const activeId = await tx.objectStore(ACTIVE_DEVICES).get(identity) as string | undefined;
      if (activeId !== undefined) {
        const stored = await tx.objectStore(DEVICES).get([...identity, activeId]) as unknown;
        const device = deviceFromSecret(stored);
        if (device.deviceId !== activeId) {
          tx.abort();
          await tx.done.catch(() => {});
          throw new E2eError('Stored encryption device identity is inconsistent');
        }
        await tx.done;
        return device;
      }
      const legacySecret = await tx.objectStore(LEGACY_DEVICES).get(pseudonym) as unknown;
      // A valid old device can be enrolled immutably; do not overwrite it or
      // silently generate a replacement when old material is malformed.
      const device = deviceFromSecret(legacySecret === undefined ? generateDeviceSecret() : legacySecret);
      await tx.objectStore(DEVICES).add(device.secret, [...identity, device.deviceId]);
      await tx.objectStore(ACTIVE_DEVICES).add(device.deviceId, identity);
      await tx.done;
      return device;
    },
    async getLegacyDeviceSecrets(serverIdentity, pseudonym) {
      const identity = scope(serverIdentity, pseudonym);
      const db = await getDatabase();
      const tx = db.transaction([DEVICES, LEGACY_DEVICES], 'readonly');
      const legacy = await tx.objectStore(LEGACY_DEVICES).get(pseudonym) as unknown;
      const stored = await tx.objectStore(DEVICES).getAll(IDBKeyRange.bound(identity, [...identity, []])) as unknown[];
      await tx.done;
      const secrets = new Map<string, Uint8Array>();
      for (const value of legacy === undefined ? stored : [legacy, ...stored]) {
        const device = deviceFromSecret(value);
        secrets.set(device.deviceId, device.secret);
      }
      return [...secrets.values()];
    },
    async replaceDevice(serverIdentity, pseudonym, expectedDeviceId) {
      const identity = scope(serverIdentity, pseudonym);
      const db = await getDatabase();
      const tx = db.transaction([DEVICES, ACTIVE_DEVICES, SIGNING, CERTIFICATES], 'readwrite');
      const activeId = await tx.objectStore(ACTIVE_DEVICES).get(identity) as string | undefined;
      if (activeId !== expectedDeviceId) {
        await tx.done;
        throw new E2eError('Another window already replaced this encryption device');
      }
      // Keep the retired secret for its historical sealed wraps.
      const previous = deviceFromSecret(await tx.objectStore(DEVICES).get([...identity, activeId]));
      if (previous.deviceId !== activeId) {
        await tx.done;
        throw new E2eError('Stored encryption device identity is inconsistent');
      }
      const device = deviceFromSecret(generateDeviceSecret());
      await tx.objectStore(DEVICES).add(device.secret, [...identity, device.deviceId]);
      const priorCertificate = await tx.objectStore(CERTIFICATES).get([...identity, activeId]) as unknown;
      await tx.objectStore(SIGNING).add({ secret: generateDeviceSecret(),
        previousCertificate: priorCertificate === undefined ? null : parseDeviceCertificate(priorCertificate),
      }, [...identity, device.deviceId]);
      await tx.objectStore(ACTIVE_DEVICES).put(device.deviceId, identity);
      await tx.done;
      return device;
    },
    async getKeys(serverIdentity, pseudonym, channelId) {
      const namespace = [...scope(serverIdentity, pseudonym), channelId];
      const db = await getDatabase();
      const values = await db.getAll(KEYS, IDBKeyRange.bound(namespace, [...namespace, []])) as unknown[];
      return values.map(retainedKey);
    },
    async putKey(serverIdentity, pseudonym, channelId, candidate) {
      const key = retainedKey(candidate);
      const namespace = [...scope(serverIdentity, pseudonym), channelId, key.purpose, key.epoch];
      const db = await getDatabase();
      const tx = db.transaction(KEYS, 'readwrite');
      const previous = await tx.store.getAll(IDBKeyRange.bound(namespace, [...namespace, []])) as unknown[];
      for (const priorValue of previous) {
        const prior = retainedKey(priorValue);
        if (prior.authorityServer !== key.authorityServer || (key.purpose === 'current' && prior.keyId !== key.keyId)) {
          tx.abort();
          await tx.done.catch(() => {});
          throw new E2eError('An immutable stored key epoch cannot be replaced');
        }
        if (prior.keyId === key.keyId && prior.verifiedLegacy) key.verifiedLegacy = true;
      }
      await tx.store.put(key, [...namespace, key.keyId]);
      await tx.done;
    },
    async getUnscopedLegacyKey(pseudonym, channelId) {
      const db = await getDatabase();
      const legacy = await db.get(LEGACY_CHANNELS, `${pseudonym}:${channelId}`) as { epoch: number; cek: unknown } | undefined;
      if (legacy === undefined) return null;
      requireSafeInteger(legacy.epoch, 1);
      return { epoch: legacy.epoch, cek: secretBytes(legacy.cek) };
    },
  };
}

export interface DeviceSigningMaterial { secret: Uint8Array; previousCertificate: DeviceCertificate | null }

/** Trust on first use is durable; a restart cannot silently accept a substituted server. */
export async function pinEncryptionServer(baseUrl: string, serverIdentity: string): Promise<void> {
  scope(serverIdentity, 'server-pin');
  const db = await getDatabase();
  const tx = db.transaction(SERVER_PINS, 'readwrite');
  const previous = await tx.store.get(baseUrl) as unknown;
  if (previous !== undefined && previous !== serverIdentity) {
    await tx.done;
    throw new E2eError('This server’s encryption identity changed. Its saved trust must be reviewed before continuing.');
  }
  if (previous === undefined) await tx.store.add(serverIdentity, baseUrl);
  await tx.done;
}

export async function getDeviceSigningMaterial(serverIdentity: string, pseudonym: string, deviceId: string): Promise<DeviceSigningMaterial> {
  const key = [...scope(serverIdentity, pseudonym), deviceId];
  const db = await getDatabase();
  const tx = db.transaction([DEVICES, SIGNING], 'readwrite');
  const device = deviceFromSecret(await tx.objectStore(DEVICES).get(key));
  if (device.deviceId !== deviceId) { await tx.done; throw new E2eError('Unknown local encryption device'); }
  const stored = await tx.objectStore(SIGNING).get(key) as DeviceSigningMaterial | undefined;
  if (stored === undefined) {
    const material = { secret: generateDeviceSecret(), previousCertificate: null };
    await tx.objectStore(SIGNING).add(material, key);
    await tx.done;
    return material;
  }
  const result = { secret: secretBytes(stored.secret),
    previousCertificate: stored.previousCertificate === null ? null : parseDeviceCertificate(stored.previousCertificate) };
  await tx.done;
  return result;
}

export async function getStoredDeviceCertificate(serverIdentity: string, pseudonym: string, deviceId: string): Promise<DeviceCertificate | null> {
  const db = await getDatabase();
  const value = await db.get(CERTIFICATES, [...scope(serverIdentity, pseudonym), deviceId]) as unknown;
  if (value === undefined) return null;
  const certificate = parseDeviceCertificate(value);
  if (certificate.server_identity !== serverIdentity || certificate.pseudonym_id !== pseudonym || certificate.device_id !== deviceId) {
    throw new E2eError('Stored device certificate has another identity context');
  }
  return certificate;
}

/** Never replace an existing certificate body; concurrent proof jobs adopt the first durable winner. */
export async function retainDeviceCertificate(certificate: DeviceCertificate): Promise<DeviceCertificate> {
  const cert = parseDeviceCertificate(certificate);
  const key = [...scope(cert.server_identity, cert.pseudonym_id), cert.device_id];
  const db = await getDatabase();
  const tx = db.transaction([CERTIFICATES, SIGNING, DEVICES], 'readwrite');
  const device = deviceFromSecret(await tx.objectStore(DEVICES).get(key));
  const signing = await tx.objectStore(SIGNING).get(key) as DeviceSigningMaterial | undefined;
  if (device.deviceId !== cert.device_id || toHex(device.publicKey) !== cert.x25519_pub_hex || !signing
    || toHex(ed25519.getPublicKey(secretBytes(signing.secret))) !== cert.ed25519_pub_hex) {
    await tx.done;
    throw new E2eError('Certificate does not match the retained device secrets');
  }
  const previous = await tx.objectStore(CERTIFICATES).get(key) as unknown;
  if (previous !== undefined) {
    const existing = parseDeviceCertificate(previous);
    await tx.done;
    if (existing.device_id !== cert.device_id || existing.ed25519_pub_hex !== cert.ed25519_pub_hex) {
      throw new E2eError('An immutable device certificate cannot be replaced');
    }
    return existing;
  }
  const predecessor = signing.previousCertificate;
  if (cert.generation !== (predecessor ? predecessor.generation + 1 : 1)
    || cert.previous_certificate_sha256 !== (predecessor ? certificateDigest(predecessor) : null)) {
    await tx.done;
    throw new E2eError('Certificate does not preserve the explicit device replacement lineage');
  }
  await tx.objectStore(CERTIFICATES).add(cert, key);
  await tx.done;
  return cert;
}
