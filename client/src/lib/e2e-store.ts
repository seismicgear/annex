/**
 * Production wiring for the E2E channel manager: an IndexedDB-backed key store
 * plus a factory that binds it to the real server API.
 *
 * Key material lives in a dedicated `annex-e2e` database (separate from the
 * identity DB) so device secrets and resolved channel keys persist across
 * restarts without ever leaving the device. The device secret is keyed by
 * pseudonym, so each identity on this machine has its own E2E key.
 */

import { openDB, type IDBPDatabase } from 'idb';
import * as e2eApi from '@/api/e2e';
import { E2eChannelManager, type E2eChannelApi, type E2eKeyStore } from './e2e-channel';

const DB_NAME = 'annex-e2e';
const DB_VERSION = 1;
const DEVICE_STORE = 'device_secrets';
const CHANNEL_STORE = 'channel_keys';

let dbPromise: Promise<IDBPDatabase> | null = null;

function getDb(): Promise<IDBPDatabase> {
  if (!dbPromise) {
    dbPromise = openDB(DB_NAME, DB_VERSION, {
      upgrade(db) {
        if (!db.objectStoreNames.contains(DEVICE_STORE)) {
          db.createObjectStore(DEVICE_STORE);
        }
        if (!db.objectStoreNames.contains(CHANNEL_STORE)) {
          db.createObjectStore(CHANNEL_STORE);
        }
      },
    });
  }
  return dbPromise;
}

/** Reset the cached connection handle (used by fresh-install cleanup). */
export function resetE2eDbHandle(): void {
  dbPromise = null;
}

/** An IndexedDB-backed {@link E2eKeyStore} scoped to one identity. */
export function indexedDbKeyStore(pseudonymId: string): E2eKeyStore {
  const chanKey = (channelId: string) => `${pseudonymId}:${channelId}`;
  return {
    async loadDeviceSecret() {
      const db = await getDb();
      const v = (await db.get(DEVICE_STORE, pseudonymId)) as Uint8Array | undefined;
      return v ?? null;
    },
    async saveDeviceSecret(secret) {
      const db = await getDb();
      await db.put(DEVICE_STORE, secret, pseudonymId);
    },
    async loadChannelKey(channelId) {
      const db = await getDb();
      const v = (await db.get(CHANNEL_STORE, chanKey(channelId))) as
        | { epoch: number; cek: Uint8Array }
        | undefined;
      return v ?? null;
    },
    async saveChannelKey(channelId, epoch, cek) {
      const db = await getDb();
      await db.put(CHANNEL_STORE, { epoch, cek }, chanKey(channelId));
    },
  };
}

/** A {@link E2eChannelApi} bound to the calling identity's pseudonym. */
export function boundE2eApi(pseudonymId: string): E2eChannelApi {
  return {
    publishMyKey: (pubHex) => e2eApi.publishMyKey(pseudonymId, pubHex),
    getChannelMemberKeys: (channelId) => e2eApi.getChannelMemberKeys(pseudonymId, channelId),
    postChannelKeyWraps: (channelId, epoch, wraps) =>
      e2eApi.postChannelKeyWraps(pseudonymId, channelId, epoch, wraps),
    getChannelKeyWraps: (channelId) => e2eApi.getChannelKeyWraps(pseudonymId, channelId),
    getChannelKeyStatus: (channelId) => e2eApi.getChannelKeyStatus(pseudonymId, channelId),
  };
}

const managers = new Map<string, E2eChannelManager>();

/**
 * Get (or lazily create) the singleton E2E manager for an identity. Cached so
 * channel keys stay warm across the app session.
 */
export function getE2eChannelManager(pseudonymId: string): E2eChannelManager {
  let mgr = managers.get(pseudonymId);
  if (!mgr) {
    mgr = new E2eChannelManager(boundE2eApi(pseudonymId), indexedDbKeyStore(pseudonymId));
    managers.set(pseudonymId, mgr);
  }
  return mgr;
}

/** Drop all in-memory managers (e.g. on logout). */
export function clearE2eManagers(): void {
  for (const m of managers.values()) m.forget();
  managers.clear();
}
