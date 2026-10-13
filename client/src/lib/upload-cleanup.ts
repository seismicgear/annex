/** Unpublished upload cleanup survives composer unmounts and interrupted sessions. */
import { authHeaders, getCredentialContext, request, StaleCredentialContextError } from '@/api/core';

export interface UploadCleanup {
  baseUrl: string; pseudonym: string; channelId: string; uploadId: string;
  pending: boolean; error: string | null;
}
const storageKey = 'annex.unpublished-upload-cleanup.v1';
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
function restore(): UploadCleanup[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(storageKey) ?? '[]');
    if (!Array.isArray(value)) return [];
    return value.filter((entry): entry is UploadCleanup => !!entry && typeof entry === 'object'
      && typeof entry.baseUrl === 'string' && typeof entry.pseudonym === 'string'
      && typeof entry.channelId === 'string' && typeof entry.uploadId === 'string' && uuid.test(entry.uploadId))
      .map(entry => ({ ...entry, pending: true, error: 'An interrupted attachment upload needs removal.' }));
  } catch { return []; }
}
let records: UploadCleanup[] = restore();
const listeners = new Set<() => void>();
const inFlight = new Map<string, Promise<void>>();
const id = (entry: UploadCleanup) => JSON.stringify([entry.baseUrl, entry.pseudonym, entry.channelId, entry.uploadId]);
function change(next: UploadCleanup[]): void {
  records = next;
  // Only public routing/owner/opaque upload IDs are persisted, never file keys,
  // filenames, content, or credentials. Memory still retains failures if storage
  // is unavailable, and the UI never reports cleanup success in that case.
  try { localStorage.setItem(storageKey, JSON.stringify(records)); } catch { /* Storage can be disabled. */ }
  for (const listener of listeners) listener();
}
export function subscribeUploadCleanup(listener: () => void): () => void {
  listeners.add(listener); return () => { listeners.delete(listener); };
}
export function getUploadCleanups(): readonly UploadCleanup[] { return records; }
export function rememberUnpublishedUpload(pseudonym: string, channelId: string, uploadId: string): UploadCleanup {
  if (!uuid.test(uploadId)) throw new Error('The attachment has an invalid upload ID.');
  const context = getCredentialContext();
  if (context.pseudonymId !== pseudonym) throw new StaleCredentialContextError('Attachment cleanup');
  const entry: UploadCleanup = { baseUrl: context.baseUrl, pseudonym, channelId, uploadId, pending: false, error: null };
  if (!records.some(existing => id(existing) === id(entry))) change([...records, entry]);
  return entry;
}
export function releaseUnpublishedUpload(entry: UploadCleanup): void {
  change(records.filter(existing => id(existing) !== id(entry)));
}
export async function removeUnpublishedUpload(entry: UploadCleanup): Promise<void> {
  const existing = inFlight.get(id(entry));
  if (existing) return existing;
  const operation = (async () => {
    try {
      const context = getCredentialContext();
      if (context.baseUrl !== entry.baseUrl || context.pseudonymId !== entry.pseudonym) {
        throw new StaleCredentialContextError('Attachment cleanup');
      }
      const result = await request<{ status: string }>(`/api/channels/${encodeURIComponent(entry.channelId)}/uploads/${encodeURIComponent(entry.uploadId)}`, {
        method: 'DELETE', headers: authHeaders(entry.pseudonym),
      });
      if (result?.status !== 'ok') throw new Error('The server did not confirm attachment removal.');
      releaseUnpublishedUpload(entry);
    } catch (error) {
      const failed = { ...entry, pending: true, error: error instanceof Error && error.message ? error.message : 'Attachment removal was not confirmed.' };
      change([...records.filter(existing => id(existing) !== id(entry)), failed]);
      throw error;
    }
  })();
  inFlight.set(id(entry), operation);
  try { await operation; } finally { inFlight.delete(id(entry)); }
}
