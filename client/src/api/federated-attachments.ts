import { authHeaders, getCredentialContext, isCredentialContextCurrent, request, StaleCredentialContextError, throwApiError } from './core';
import { encryptedUploadGrant } from './encrypted-uploads';
import { transportFetch } from './hosted-fetch';
import { getE2eControls } from '@/lib/e2e-store';

export type OrdinaryAttachmentCategory = 'image' | 'video' | 'file';
export interface OrdinaryAttachment { id: string; path: string; filename: string; category: OrdinaryAttachmentCategory }
const MAX_BYTES = 50 * 1024 * 1024;
const MIME: Record<string, string> = { jpg: 'image/jpeg', jpeg: 'image/jpeg', png: 'image/png', gif: 'image/gif', webp: 'image/webp',
  mp4: 'video/mp4', webm: 'video/webm', mov: 'video/quicktime' };

/** Ordinary messages carry only a canonical local replica path. Never forward
 * its grant to a URL supplied in message text or treat an encrypted .bin as an
 * ordinary image. Remote origins must deliver their bytes to this replica.
 */
export function parseOrdinaryAttachment(path: string, category: OrdinaryAttachmentCategory): OrdinaryAttachment {
  const match = /^\/uploads\/chat\/(images|videos|files)\/([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\.([a-z0-9]{1,16})$/.exec(path);
  if (!match || match[1] !== `${category}s`) throw new Error('This attachment does not have a valid local replica path.');
  const mime = MIME[match[3]];
  if (category !== 'file' && !mime?.startsWith(`${category}/`)) throw new Error('This attachment has an unsupported media format.');
  return { id: match[2], path, filename: `${match[2]}.${match[3]}`, category };
}
function capture(pseudonym: string, signal?: AbortSignal) {
  const context = getCredentialContext();
  const check = () => {
    signal?.throwIfAborted();
    if (context.pseudonymId !== pseudonym || !isCredentialContextCurrent(context)) throw new StaleCredentialContextError('Federated attachment');
  };
  check(); return { context, check };
}
export async function readOrdinaryFederatedAttachment(pseudonym: string, path: string, category: OrdinaryAttachmentCategory, signal: AbortSignal): Promise<Blob> {
  const attachment = parseOrdinaryAttachment(path, category), { context, check } = capture(pseudonym, signal);
  const device = await getE2eControls(pseudonym).ensureCertifiedDevice(); check();
  // This grant is device-bound but confers no authority by itself. The storage
  // service verifies current authority membership again for every actual read.
  const token = await encryptedUploadGrant(pseudonym, device.device_id); check();
  const response = await transportFetch(`${context.baseUrl}${attachment.path}?t=${encodeURIComponent(token)}`, { signal, redirect: 'error' });
  if (!response.ok) await throwApiError(response);
  check();
  const sizeHeader = response.headers.get('Content-Length');
  const declared = sizeHeader === null ? null : /^\d+$/.test(sizeHeader) ? Number(sizeHeader) : NaN;
  if (declared !== null && (!Number.isSafeInteger(declared) || declared < 1 || declared > MAX_BYTES)) throw new Error('The attachment has an unsupported download size.');
  const mime = response.headers.get('Content-Type')?.split(';')[0].trim().toLowerCase();
  const expected = MIME[attachment.filename.split('.').at(-1)!];
  if (category !== 'file' && mime !== expected) throw new Error('The attachment media type does not match its accepted path.');
  if (!response.body) throw new Error('The attachment response has no body.');
  const reader = response.body.getReader(), chunks: ArrayBuffer[] = [];
  let received = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read(); check();
      if (done) break;
      received += value.length;
      if (received > MAX_BYTES || (declared !== null && received > declared)) throw new Error('The attachment exceeds its declared download size.');
      chunks.push(value.slice().buffer);
    }
  } finally { await reader.cancel().catch(() => {}); }
  if (declared !== null && received !== declared) throw new Error('The attachment download is incomplete.');
  if (received === 0) throw new Error('The attachment download is empty.');
  // There is no AEAD nonce/tag in ordinary files: one to 27-byte plaintext must
  // not inherit the encrypted transfer minimum or overhead subtraction.
  return new Blob(chunks, { type: category === 'file' ? 'application/octet-stream' : expected });
}

export async function retryOrdinaryFederatedAttachment(pseudonym: string, channelId: string, attachmentId: string): Promise<void> {
  const { check } = capture(pseudonym);
  const device = await getE2eControls(pseudonym).ensureCertifiedDevice(); check();
  const result = await request<{ status: string; attachment_id: string }>(`/api/channels/${encodeURIComponent(channelId)}/uploads/${encodeURIComponent(attachmentId)}/retry`, {
    method: 'POST', headers: authHeaders(pseudonym), body: JSON.stringify({ device_id: device.device_id }),
  }); check();
  if (result.attachment_id !== attachmentId || !['pending', 'ready'].includes(result.status)) throw new Error('The server did not confirm this attachment retry.');
}
