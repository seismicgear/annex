/**
 * Server hub persistence — IndexedDB storage for saved server connections.
 *
 * Each entry represents an established Merkle tree insertion on a remote
 * server. The visual "server list" is a rendering of this local database.
 */

import { openDB, type IDBPDatabase } from 'idb';
import type { SavedServer, ServerSummary } from '@/types';

const DB_NAME = 'annex-servers';
const DB_VERSION = 1;
const SERVER_STORE = 'servers';

let dbPromise: Promise<IDBPDatabase> | null = null;

function getDb(): Promise<IDBPDatabase> {
  if (!dbPromise) {
    dbPromise = openDB(DB_NAME, DB_VERSION, {
      upgrade(db) {
        if (!db.objectStoreNames.contains(SERVER_STORE)) {
          const store = db.createObjectStore(SERVER_STORE, { keyPath: 'id' });
          store.createIndex('identityId', 'identityId', { unique: false });
          store.createIndex('slug', 'slug', { unique: false });
        }
      },
    });
  }
  return dbPromise;
}

const ACCENT_COLORS = [
  '#646cff', '#e63946', '#f87171', '#fbbf24', '#7eb8da',
  '#b87eda', '#ff6b9d', '#c42836', '#6366f1', '#ec4899',
];

/** Pick a random accent color for a new server entry. */
export function randomAccentColor(): string {
  return ACCENT_COLORS[Math.floor(Math.random() * ACCENT_COLORS.length)];
}

/** Save or update a server entry. */
export async function saveServer(server: SavedServer): Promise<void> {
  const db = await getDb();
  await db.put(SERVER_STORE, server);
}

/** Get a saved server by ID. */
export async function getServer(id: string): Promise<SavedServer | undefined> {
  const db = await getDb();
  return db.get(SERVER_STORE, id);
}

/** List all saved servers, sorted by most recently connected. */
export async function listServers(): Promise<SavedServer[]> {
  const db = await getDb();
  const all = await db.getAll(SERVER_STORE);
  return all.sort(
    (a, b) => new Date(b.lastConnectedAt).getTime() - new Date(a.lastConnectedAt).getTime(),
  );
}

/** Find saved server by identity ID. */
export async function getServerByIdentityId(identityId: string): Promise<SavedServer | undefined> {
  const db = await getDb();
  const matches = await db.getAllFromIndex(SERVER_STORE, 'identityId', identityId);
  return matches[0];
}

/** Find saved server by slug. */
export async function getServerBySlug(slug: string): Promise<SavedServer | undefined> {
  const db = await getDb();
  const matches = await db.getAllFromIndex(SERVER_STORE, 'slug', slug);
  return matches[0];
}

/** Remove a saved server. */
export async function removeServer(id: string): Promise<void> {
  const db = await getDb();
  await db.delete(SERVER_STORE, id);
}

/** Update cached summary for a saved server. */
export async function updateCachedSummary(
  id: string,
  summary: ServerSummary,
): Promise<void> {
  const db = await getDb();
  const server = await db.get(SERVER_STORE, id);
  if (server) {
    server.cachedSummary = summary;
    await db.put(SERVER_STORE, server);
  }
}

/** Create a new SavedServer entry for an identity that just registered. */
export function createServerEntry(
  baseUrl: string,
  slug: string,
  label: string,
  identityId: string,
  accentColor?: string,
): SavedServer {
  return {
    id: crypto.randomUUID(),
    baseUrl,
    slug,
    label,
    identityId,
    personaId: null,
    accentColor: accentColor ?? randomAccentColor(),
    vrpTopic: `annex:server:${slug}:v1`,
    lastConnectedAt: new Date().toISOString(),
    cachedSummary: null,
  };
}
