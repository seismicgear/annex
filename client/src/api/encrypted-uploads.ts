import { authHeaders, fetchAuthenticated, getCredentialContext, isCredentialContextCurrent, getSessionToken, request, throwApiError, StaleCredentialContextError } from './core';
import { getUploadPolicy } from './uploads';
import { transportFetch } from './hosted-fetch';
import { getE2eChannelManager } from '@/lib/e2e-store';
import { beginLiveOperation } from '@/lib/session-ownership';
import { parsePrivateAttachment, preparePrivateAttachment, openPrivateAttachment, type PrivateAttachment } from '@/lib/encrypted-attachments';
import { E2eError } from '@/lib/e2e';
import { rememberUnpublishedUpload, releaseUnpublishedUpload, removeUnpublishedUpload, type UploadCleanup } from '@/lib/upload-cleanup';

function capture(pseudonym: string, signal?: AbortSignal) {
  const context = getCredentialContext();
  const check = () => {
    signal?.throwIfAborted();
    if (context.pseudonymId !== pseudonym || !isCredentialContextCurrent(context)) throw new StaleCredentialContextError('Attachment operation');
  };
  check();
  return { context, check };
}
export async function abortStagedUpload(pseudonym: string, channelId: string, uploadId: string): Promise<void> {
  await removeUnpublishedUpload(rememberUnpublishedUpload(pseudonym, channelId, uploadId));
}
export async function uploadEncryptedAttachment(pseudonym: string, channelId: string, file: File, signal?: AbortSignal): Promise<PrivateAttachment> {
  const { context, check } = capture(pseudonym, signal);
  const operation = beginLiveOperation(context.baseUrl, pseudonym);
  const joinedSignal = signal ? AbortSignal.any([signal, operation.signal]) : operation.signal;
  let cleanup: UploadCleanup | null = null;
  try {
    const policy = await getUploadPolicy(pseudonym, channelId); check(); joinedSignal.throwIfAborted();
    if (file.size > 50 * 1024 * 1024) throw new E2eError('This file exceeds the upload size limit.');
    const bytes = new Uint8Array(await file.arrayBuffer()); check(); joinedSignal.throwIfAborted();
    const key = await getE2eChannelManager(pseudonym).resolveChannelKey(channelId); check(); joinedSignal.throwIfAborted();
    const prepared = preparePrivateAttachment(bytes, file.name, file.type, policy, key);
    const a = prepared.descriptor;
    const form = new FormData();
    form.append('attachment_id', a.id); form.append('epoch', String(a.epoch)); form.append('key_id', a.keyId); form.append('category', a.category);
    form.append('file', new Blob([prepared.ciphertext.slice().buffer], { type: 'application/octet-stream' }), `${a.id}.bin`);
    // The UUID is known before the POST. A lost response does not erase the
    // ability to cancel a upload that the server has already committed.
    cleanup = rememberUnpublishedUpload(pseudonym, channelId, a.id);
    const response = await fetchAuthenticated(`${context.baseUrl}/api/channels/${encodeURIComponent(channelId)}/encrypted-upload`, {
      method: 'POST', headers: authHeaders(pseudonym), body: form, signal: joinedSignal,
    });
    if (!response.ok) await throwApiError(response);
    const result = await response.json(); check(); joinedSignal.throwIfAborted();
    if (result.status !== 'ok' || result.upload_id !== a.id || result.url !== a.url || result.encrypted !== true
      || result.epoch !== a.epoch || result.key_id !== a.keyId || result.category !== a.category
      || result.ciphertext_sha256 !== a.ciphertextSha256 || result.size !== prepared.ciphertext.length
      || result.filename !== 'encrypted.bin' || result.content_type !== 'application/octet-stream') throw new E2eError('The server did not confirm the encrypted attachment bytes.');
    const descriptor = parsePrivateAttachment({ ...a, originIdentity: result.origin_identity, originatingServer: result.originating_server });
    releaseUnpublishedUpload(cleanup);
    return descriptor;
  } catch (error) {
    if (cleanup) {
      // Use the independent DELETE signal, not the already-aborted upload.
      // Failure remains in the durable cleanup queue with an explicit retry.
      await removeUnpublishedUpload(cleanup).catch(() => {});
    }
    throw error;
  } finally { operation.release(); }
}

// Separate from anonymous/local image grants: an encrypted read must never
// downgrade to a v1 token or fetch an ungranted URL when issuance fails.
let grantScope = '';
const grants = new Map<string, { token: string; expires: number }>();
const pendingGrants = new Map<string, Promise<string>>();
export async function encryptedUploadGrant(pseudonym: string, deviceId: string): Promise<string> {
  if (!/^[0-9a-f]{64}$/.test(deviceId)) throw new E2eError('Invalid attachment device.');
  const { context, check } = capture(pseudonym);
  const token = getSessionToken();
  const scope = JSON.stringify([context.serial, token]);
  if (grantScope !== scope) { grants.clear(); pendingGrants.clear(); grantScope = scope; }
  const existing = grants.get(deviceId);
  if (existing && Date.now() < existing.expires - 60_000) return existing.token;
  const current = pendingGrants.get(deviceId);
  if (current) return current;
  const promise = (async () => {
    const result = await request<{ token: string; expiresInSecs: number }>('/api/uploads/grant', {
      method: 'POST', headers: authHeaders(pseudonym), body: JSON.stringify({ device_id: deviceId }),
    });
    check();
    if (grantScope !== scope || getSessionToken() !== token) throw new StaleCredentialContextError('Attachment grant');
    if (typeof result.token !== 'string' || !result.token.length || !Number.isSafeInteger(result.expiresInSecs) || result.expiresInSecs <= 0) throw new E2eError('Invalid attachment grant.');
    grants.set(deviceId, { token: result.token, expires: Date.now() + result.expiresInSecs * 1000 });
    return result.token;
  })();
  pendingGrants.set(deviceId, promise);
  try { return await promise; }
  finally { if (pendingGrants.get(deviceId) === promise) pendingGrants.delete(deviceId); }
}
export async function readEncryptedAttachment(pseudonym: string, channelId: string, attachment: PrivateAttachment, signal: AbortSignal): Promise<Blob> {
  const a = parsePrivateAttachment(attachment);
  const { context, check } = capture(pseudonym, signal);
  const device = await getE2eChannelManager(pseudonym).ensureDevicePublished(channelId); check();
  const token = await encryptedUploadGrant(pseudonym, device); check();
  const response = await transportFetch(`${context.baseUrl}${a.url}?t=${encodeURIComponent(token)}`, { signal, redirect: 'error' });
  if (!response.ok) await throwApiError(response);
  const expected = a.bytes + 28;
  const declared = response.headers.get('Content-Length');
  if (declared !== null && Number(declared) !== expected) throw new E2eError('Encrypted attachment length changed.');
  if (!response.body) throw new E2eError('Encrypted attachment has no body.');
  const ciphertext = new Uint8Array(expected);
  const reader = response.body.getReader();
  let offset = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read(); check();
      if (done) break;
      if (offset + value.length > expected) throw new E2eError('Encrypted attachment exceeds its authenticated size.');
      ciphertext.set(value, offset); offset += value.length;
    }
  } finally { await reader.cancel().catch(() => {}); }
  if (offset !== expected) throw new E2eError('Encrypted attachment is incomplete.');
  const plaintext = openPrivateAttachment(ciphertext, a, channelId); check();
  return new Blob([plaintext.slice().buffer], { type: a.category === 'file' ? 'application/octet-stream' : a.contentType });
}
export async function retryEncryptedAttachment(pseudonym: string, channelId: string, attachmentId: string): Promise<void> {
  const { check } = capture(pseudonym);
  const device = await getE2eChannelManager(pseudonym).ensureDevicePublished(channelId); check();
  const result = await request<{ status: string; attachment_id: string }>(`/api/channels/${encodeURIComponent(channelId)}/uploads/${encodeURIComponent(attachmentId)}/retry`, {
    method: 'POST', headers: authHeaders(pseudonym), body: JSON.stringify({ device_id: device }),
  }); check();
  if (result.attachment_id !== attachmentId || !['pending','ready'].includes(result.status)) throw new E2eError('The attachment retry was not confirmed.');
}
