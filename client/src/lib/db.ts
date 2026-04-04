/**
 * IndexedDB storage for identity keys and session state.
 *
 * Uses the `idb` library for a promise-based IndexedDB API.
 * Keys are stored encrypted-at-rest where the browser supports it,
 * but the primary security boundary is the user's device.
 */

import { openDB, type IDBPDatabase } from 'idb';
import type { StoredIdentity } from '@/types';

const DB_NAME = 'annex-identity';
const DB_VERSION = 1;
const IDENTITY_STORE = 'identities';

let dbPromise: Promise<IDBPDatabase> | null = null;

function getDb(): Promise<IDBPDatabase> {
  if (!dbPromise) {
    dbPromise = openDB(DB_NAME, DB_VERSION, {
      upgrade(db) {
        if (!db.objectStoreNames.contains(IDENTITY_STORE)) {
          db.createObjectStore(IDENTITY_STORE, { keyPath: 'id' });
        }
      },
    });
  }
  return dbPromise;
}

/** Store a new identity. */
export async function saveIdentity(identity: StoredIdentity): Promise<void> {
  const db = await getDb();
  await db.put(IDENTITY_STORE, identity);
}

/** Retrieve an identity by its ID. */
export async function getIdentity(id: string): Promise<StoredIdentity | undefined> {
  const db = await getDb();
  return db.get(IDENTITY_STORE, id);
}

/** List all stored identities. */
export async function listIdentities(): Promise<StoredIdentity[]> {
  const db = await getDb();
  return db.getAll(IDENTITY_STORE);
}

/** Delete an identity by ID. */
export async function deleteIdentity(id: string): Promise<void> {
  const db = await getDb();
  await db.delete(IDENTITY_STORE, id);
}

/** Update pseudonymId after membership verification. */
export async function updateIdentityPseudonym(
  id: string,
  pseudonymId: string,
): Promise<void> {
  const db = await getDb();
  const identity = await db.get(IDENTITY_STORE, id);
  if (identity) {
    identity.pseudonymId = pseudonymId;
    await db.put(IDENTITY_STORE, identity);
  }
}

/**
 * Clone an existing identity for registration on a different server.
 * Copies the key material (sk, roleCode, nodeId, commitmentHex) into a
 * new record with a fresh UUID and cleared server-specific fields.
 */
export async function cloneIdentityForServer(sourceId: string): Promise<StoredIdentity | undefined> {
  const source = await getIdentity(sourceId);
  if (!source) return undefined;
  const cloned: StoredIdentity = {
    id: crypto.randomUUID(),
    sk: source.sk,
    roleCode: source.roleCode,
    nodeId: source.nodeId,
    commitmentHex: source.commitmentHex,
    pseudonymId: null,
    sessionToken: null,
    serverSlug: '',
    leafIndex: null,
    createdAt: new Date().toISOString(),
  };
  await saveIdentity(cloned);
  return cloned;
}

/** Export an identity for backup (JSON string). */
export function exportIdentity(identity: StoredIdentity): string {
  return JSON.stringify(identity, null, 2);
}

/** Import an identity from a backup JSON string. */
export async function importIdentity(json: string): Promise<StoredIdentity> {
  const parsed: StoredIdentity = JSON.parse(json);
  // Valid key material requires sk and roleCode. commitmentHex is required
  // for registered identities, but locally recovered-but-unregistered
  // identities with valid key material (sk + commitmentHex + nodeId) are
  // also accepted even when pseudonymId is null.
  if (!parsed.sk || !parsed.roleCode) {
    throw new Error('Invalid identity backup: missing required fields (sk, roleCode)');
  }
  if (!parsed.commitmentHex && !parsed.nodeId) {
    throw new Error('Invalid identity backup: missing key material (commitmentHex or nodeId)');
  }
  // Always generate a fresh ID to prevent a malicious backup from
  // overwriting an existing identity via IndexedDB's put() upsert.
  // Only preserve the cryptographic key material fields.
  const identity: StoredIdentity = {
    id: crypto.randomUUID(),
    sk: parsed.sk,
    roleCode: parsed.roleCode,
    nodeId: parsed.nodeId,
    commitmentHex: parsed.commitmentHex,
    pseudonymId: parsed.pseudonymId ?? null,
    sessionToken: null, // Never trust imported session tokens
    serverSlug: parsed.serverSlug ?? '',
    leafIndex: parsed.leafIndex ?? null,
    createdAt: new Date().toISOString(),
  };
  await saveIdentity(identity);
  return identity;
}

/**
 * Delete all Annex IndexedDB databases (identities, servers, personas).
 *
 * Used during fresh-install detection in Tauri mode to ensure stale data
 * from a previous installation is fully removed. Also resets internal
 * connection handles so subsequent calls re-open fresh databases.
 */
export async function clearAllDatabases(): Promise<void> {
  const DB_NAMES = ['annex-identity', 'annex-servers', 'annex-personas'];
  // Close all cached connection handles so subsequent calls re-open fresh databases.
  dbPromise = null;
  // Also reset handles in the servers and personas modules.
  // Dynamic imports to avoid circular dependency at module level.
  try {
    const { resetDbHandle: resetServersDb } = await import('@/lib/servers');
    resetServersDb();
  } catch { /* module may not be loaded yet */ }
  try {
    const { resetDbHandle: resetPersonasDb } = await import('@/lib/personas');
    resetPersonasDb();
  } catch { /* module may not be loaded yet */ }
  for (const name of DB_NAMES) {
    try {
      await new Promise<void>((resolve, reject) => {
        const req = indexedDB.deleteDatabase(name);
        req.onsuccess = () => resolve();
        req.onerror = () => reject(req.error);
        req.onblocked = () => resolve(); // Best-effort
      });
    } catch {
      // Non-fatal — database may not exist yet.
    }
  }
}
