/** Private attachment descriptors exist only inside authenticated message bodies. */
import { sha256 } from '@noble/hashes/sha2.js';
import { E2eError, toHex, generateChannelKey } from './e2e';
import { decodeHex32, encryptAttachment, decryptAttachment, type EpochContext } from './e2e-v2';
import { stripImageMetadata } from './image-privacy';
import type { UploadPolicy } from '@/api/uploads';

export const PRIVATE_MESSAGE_PREFIX = 'annex.message.v1:';
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
const HASH = /^[0-9a-f]{64}$/;
const groups = { image: 'images', video: 'videos', file: 'files' } as const;
function unsafeFilenameCharacter(char: string): boolean { const code = char.charCodeAt(0); return code < 32 || code === 127 || char === '/' || char === '\\'; }
export type AttachmentCategory = keyof typeof groups;
export interface PrivateAttachment {
  id: string; url: string; name: string; contentType: string; category: AttachmentCategory;
  bytes: number; plaintextSha256: string; ciphertextSha256: string; fileKeyHex: string;
  epoch: number; keyId: string; authorityNamespace: string; channelId: string;
  originIdentity: string; originatingServer: string;
}
export interface PrivateMessage { text: string; attachments: PrivateAttachment[] }
function invalid(): never { throw new E2eError('The encrypted attachment descriptor is invalid.'); }
export function attachmentPath(id: string, category: AttachmentCategory): string {
  if (!UUID.test(id) || !(category in groups)) return invalid();
  return `/uploads/chat/${groups[category]}/${id}.bin`;
}
export function parsePrivateAttachment(value: unknown): PrivateAttachment {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return invalid();
  const a = value as PrivateAttachment;
  if (typeof a.id !== 'string' || !UUID.test(a.id) || !['image','video','file'].includes(a.category)
    || a.url !== attachmentPath(a.id, a.category) || typeof a.name !== 'string' || !a.name.length || a.name.length > 255 || [...a.name].some(unsafeFilenameCharacter)
    || typeof a.contentType !== 'string' || a.contentType.length > 100 || !Number.isSafeInteger(a.bytes) || a.bytes < 0 || a.bytes > 50 * 1024 * 1024
    || !HASH.test(a.plaintextSha256) || !HASH.test(a.ciphertextSha256) || !HASH.test(a.fileKeyHex) || !HASH.test(a.keyId)
    || !Number.isSafeInteger(a.epoch) || a.epoch < 1 || typeof a.authorityNamespace !== 'string' || !a.authorityNamespace.length || a.authorityNamespace.length > 2048
    || typeof a.channelId !== 'string' || !a.channelId.length || a.channelId.length > 512 || !/^annex-server:[0-9a-f]{64}$/.test(a.originIdentity)) return invalid();
  try { const origin = new URL(a.originatingServer); if (!['https:', 'http:'].includes(origin.protocol) || origin.origin !== a.originatingServer) return invalid(); } catch { return invalid(); }
  const supported = a.category === 'image' ? ['image/jpeg','image/png','image/gif','image/webp']
    : a.category === 'video' ? ['video/mp4','video/webm','video/quicktime'] : ['text/plain','application/pdf','application/zip','application/octet-stream'];
  if (!supported.includes(a.contentType)) return invalid();
  return { id: a.id, url: a.url, name: a.name, contentType: a.contentType, category: a.category, bytes: a.bytes,
    plaintextSha256: a.plaintextSha256, ciphertextSha256: a.ciphertextSha256, fileKeyHex: a.fileKeyHex,
    epoch: a.epoch, keyId: a.keyId, authorityNamespace: a.authorityNamespace, channelId: a.channelId,
    originIdentity: a.originIdentity, originatingServer: a.originatingServer };
}
export function parsePrivateMessage(content: string): PrivateMessage | null {
  if (!content.startsWith(PRIVATE_MESSAGE_PREFIX)) return null;
  try {
    const value = JSON.parse(content.slice(PRIVATE_MESSAGE_PREFIX.length)) as PrivateMessage;
    if (!value || typeof value.text !== 'string' || !Array.isArray(value.attachments) || value.attachments.length > 16) return invalid();
    const attachments = value.attachments.map(parsePrivateAttachment);
    if (new Set(attachments.map(a => a.id)).size !== attachments.length) return invalid();
    return { text: value.text, attachments };
  } catch { return invalid(); }
}
export function privateMessage(text: string, attachments: PrivateAttachment[]): string {
  return PRIVATE_MESSAGE_PREFIX + JSON.stringify({ text, attachments: attachments.map(parsePrivateAttachment) });
}
/** Never expose a private descriptor or key in reply text, history, or an error. */
export function messageText(content: string): string {
  try { const parsed = parsePrivateMessage(content); return parsed ? parsed.text || 'Encrypted attachment' : content; }
  catch { return 'Encrypted attachment could not be verified'; }
}

export function detectAttachment(bytes: Uint8Array, declaredType: string): { contentType: string; category: AttachmentCategory } {
  const ascii = (start: number, length: number) => String.fromCharCode(...bytes.subarray(start, start + length));
  const start = (...values: number[]) => values.every((v, i) => bytes[i] === v);
  if (start(0x7f,0x45,0x4c,0x46) || ascii(0,2) === 'MZ') throw new E2eError('Executable files cannot be uploaded.');
  let type: string;
  if (start(255,216,255)) type = 'image/jpeg';
  else if (start(137,80,78,71,13,10,26,10)) type = 'image/png';
  else if (['GIF87a','GIF89a'].includes(ascii(0,6))) type = 'image/gif';
  else if (ascii(0,4) === 'RIFF' && ascii(8,4) === 'WEBP') type = 'image/webp';
  else if (ascii(4,4) === 'ftyp' && bytes.length >= 12) type = ascii(8,4) === 'qt  ' ? 'video/quicktime' : 'video/mp4';
  else if (start(0x1a,0x45,0xdf,0xa3)) type = 'video/webm';
  else if (ascii(0,4) === '%PDF') type = 'application/pdf';
  else if (start(80,75,3,4)) type = 'application/zip';
  else if (declaredType === 'text/plain') {
    try { new TextDecoder('utf-8', { fatal: true }).decode(bytes); } catch { throw new E2eError('The selected text file is not valid UTF-8.'); }
    if (bytes.includes(0)) throw new E2eError('The selected file contains binary data.');
    type = 'text/plain';
  } else if (!declaredType || declaredType === 'application/octet-stream') type = 'application/octet-stream';
  else throw new E2eError('The file contents do not match a supported image, video or file format.');
  return { contentType: type, category: type.startsWith('image/') ? 'image' : type.startsWith('video/') ? 'video' : 'file' };
}
export function preparePrivateAttachment(bytes: Uint8Array, name: string, declaredType: string, policy: UploadPolicy,
  context: EpochContext, id = crypto.randomUUID()): { ciphertext: Uint8Array; descriptor: Omit<PrivateAttachment, 'originIdentity' | 'originatingServer'> } {
  const { contentType, category } = detectAttachment(bytes, declaredType);
  const enabled = category === 'image' ? policy.images_enabled : category === 'video' ? policy.videos_enabled : policy.files_enabled;
  const maximum = category === 'image' ? policy.max_image_size_mb : category === 'video' ? policy.max_video_size_mb : policy.max_file_size_mb;
  if (!enabled) throw new E2eError(`${category} uploads are disabled on this server.`);
  if (bytes.length > Math.min(maximum, 50) * 1024 * 1024) throw new E2eError(`This ${category} exceeds the server's size limit.`);
  const clean = category === 'image' ? stripImageMetadata(bytes, contentType) : bytes;
  const key = generateChannelKey();
  try {
    const ciphertext = encryptAttachment(clean, key, context, id);
    return { ciphertext, descriptor: { id, url: attachmentPath(id, category), name: [...name].map(char => unsafeFilenameCharacter(char) ? '_' : char).join('').slice(0,255) || 'download',
      contentType, category, bytes: clean.length, plaintextSha256: toHex(sha256(clean)), ciphertextSha256: toHex(sha256(ciphertext)), fileKeyHex: toHex(key),
      epoch: context.epoch, keyId: context.keyId, authorityNamespace: context.authorityServer, channelId: context.channelId } };
  } finally { key.fill(0); }
}
export function openPrivateAttachment(ciphertext: Uint8Array, descriptor: PrivateAttachment, channelId: string): Uint8Array {
  const a = parsePrivateAttachment(descriptor);
  if (a.channelId !== channelId) throw new E2eError('This attachment belongs to another channel.');
  const key = decodeHex32(a.fileKeyHex);
  try {
    const plaintext = decryptAttachment(ciphertext, key, { authorityServer: a.authorityNamespace, channelId, epoch: a.epoch, keyId: a.keyId }, a.id, a.ciphertextSha256, a.bytes);
    if (toHex(sha256(plaintext)) !== a.plaintextSha256) throw new E2eError('The decrypted attachment digest does not match.');
    const detected = detectAttachment(plaintext, a.contentType);
    if (detected.contentType !== a.contentType || detected.category !== a.category) throw new E2eError('The decrypted file type differs from its description.');
    return plaintext;
  } finally { key.fill(0); }
}
