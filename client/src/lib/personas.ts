/**
 * Persona management — local persona profiles stored in IndexedDB.
 *
 * Personas are user-defined display identities that map to server-scoped
 * pseudonyms. A user might be "seismicgear" on a gaming server, "Jane Doe"
 * on a professional node, and anonymous on a civic governance server.
 *
 * The persona is purely a client-side abstraction; the server never sees
 * display names or avatars.
 */

import { openDB, type IDBPDatabase } from 'idb';
import type { Persona } from '@/types';

const DB_NAME = 'annex-personas';
const DB_VERSION = 1;
const PERSONA_STORE = 'personas';

let dbPromise: Promise<IDBPDatabase> | null = null;

function getDb(): Promise<IDBPDatabase> {
  if (!dbPromise) {
    dbPromise = openDB(DB_NAME, DB_VERSION, {
      upgrade(db) {
        if (!db.objectStoreNames.contains(PERSONA_STORE)) {
          const store = db.createObjectStore(PERSONA_STORE, { keyPath: 'id' });
          store.createIndex('identityId', 'identityId', { unique: false });
        }
      },
    });
  }
  return dbPromise;
}

export const ACCENT_COLORS = [
  '#e63946', '#646cff', '#4ade80', '#f87171', '#fbbf24', '#7eb8da',
  '#b87eda', '#ff6b9d', '#c42836', '#10b981', '#6366f1', '#ec4899',
];

/** Generate a random accent color for a new persona. */
export function randomAccentColor(): string {
  return ACCENT_COLORS[Math.floor(Math.random() * ACCENT_COLORS.length)];
}

/** Create and store a new persona. */
export async function createPersona(
  displayName: string,
  identityId: string,
  serverSlug: string,
  bio = '',
  avatarUrl: string | null = null,
  accentColor?: string,
): Promise<Persona> {
  const persona: Persona = {
    id: crypto.randomUUID(),
    displayName,
    avatarUrl,
    identityId,
    serverSlug,
    bio,
    accentColor: accentColor ?? randomAccentColor(),
    createdAt: new Date().toISOString(),
  };
  const db = await getDb();
  await db.put(PERSONA_STORE, persona);
  return persona;
}

/** Get all personas for a given identity. */
export async function getPersonasForIdentity(identityId: string): Promise<Persona[]> {
  const db = await getDb();
  return db.getAllFromIndex(PERSONA_STORE, 'identityId', identityId);
}

/** Get all stored personas. */
export async function listPersonas(): Promise<Persona[]> {
  const db = await getDb();
  return db.getAll(PERSONA_STORE);
}

/** Get a single persona by ID. */
export async function getPersona(id: string): Promise<Persona | undefined> {
  const db = await getDb();
  return db.get(PERSONA_STORE, id);
}

/** Update an existing persona. */
export async function updatePersona(persona: Persona): Promise<void> {
  const db = await getDb();
  await db.put(PERSONA_STORE, persona);
}

/** Delete a persona. */
export async function deletePersona(id: string): Promise<void> {
  const db = await getDb();
  await db.delete(PERSONA_STORE, id);
}

/**
 * Resolve the display name for a given identity.
 * Returns the persona display name if one exists, otherwise the truncated pseudonym.
 */
export async function resolveDisplayName(
  identityId: string,
  pseudonymId: string | null,
): Promise<string> {
  const personas = await getPersonasForIdentity(identityId);
  if (personas.length > 0) {
    return personas[0].displayName;
  }
  return pseudonymId ? pseudonymId.slice(0, 12) + '...' : 'Anonymous';
}
