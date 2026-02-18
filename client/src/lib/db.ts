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

/** Export an identity for backup (JSON string). */
export function exportIdentity(identity: StoredIdentity): string {
  return JSON.stringify(identity, null, 2);
}

/** Import an identity from a backup JSON string. */
export async function importIdentity(json: string): Promise<StoredIdentity> {
  const identity: StoredIdentity = JSON.parse(json);
  if (!identity.sk || !identity.commitmentHex || !identity.roleCode) {
    throw new Error('Invalid identity backup: missing required fields');
  }
  await saveIdentity(identity);
  return identity;
}
