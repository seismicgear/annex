/** Context-authenticated device wraps and epoch-addressed content envelopes. */
import { x25519 } from '@noble/curves/ed25519.js';
import { sha256 } from '@noble/hashes/sha2.js';
import {
  decryptContent, E2eError, encryptContent, fromBase64, openFrom,
  publicKeyFromSecret, sealTo, toBase64, toHex, utf8,
} from './e2e';

export const E2E_V2_PREFIX = 'e2e2:';
const KEY_DOMAIN = utf8.encode('annex-channel-key-v2\0');
const LOWER_HEX_32 = /^[0-9a-f]{64}$/;
const BASE64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

export interface EpochContext {
  authorityServer: string;
  channelId: string;
  epoch: number;
  keyId: string;
}

export interface WrapContext extends EpochContext {
  recipientPseudonym: string;
  recipientDeviceId: string;
}

export type WrapPurpose = 'current' | 'legacy';

export function requireSafeInteger(value: number, minimum = 0): number {
  if (!Number.isSafeInteger(value) || value < minimum) throw new E2eError('Invalid key epoch or revision');
  return value;
}

export function decodeHex32(value: string): Uint8Array {
  if (!LOWER_HEX_32.test(value)) throw new E2eError('Expected 32 bytes of canonical lowercase hex');
  return Uint8Array.from({ length: 32 }, (_, index) => Number.parseInt(value.slice(index * 2, index * 2 + 2), 16));
}

export function decodeCanonicalBase64(value: string, maximumBytes = Number.MAX_SAFE_INTEGER): Uint8Array {
  if (typeof value !== 'string' || value.length > Math.ceil(maximumBytes / 3) * 4 || !BASE64.test(value)) {
    throw new E2eError('Invalid canonical base64');
  }
  const decoded = fromBase64(value);
  if (decoded.length > maximumBytes || toBase64(decoded) !== value) throw new E2eError('Invalid canonical base64');
  return decoded;
}

export function decodeUtf8Strict(value: Uint8Array): string {
  try { return new TextDecoder('utf-8', { fatal: true }).decode(value); }
  catch { throw new E2eError('Encrypted content is not valid UTF-8'); }
}

function requireText(value: string, maximum: number): void {
  if (typeof value !== 'string' || !value.length || value.length > maximum
    || decodeUtf8Strict(utf8.encode(value)) !== value) throw new E2eError('Invalid encryption context');
}

function requireEpochContext(context: EpochContext): void {
  requireText(context.authorityServer, 2048);
  requireText(context.channelId, 256);
  requireSafeInteger(context.epoch, 1);
  decodeHex32(context.keyId);
}

function requireWrapContext(context: WrapContext): void {
  requireEpochContext(context);
  requireText(context.recipientPseudonym, 512);
  decodeHex32(context.recipientDeviceId);
}

/** Public identifier of immutable X25519 material; rejects low-order keys. */
export function deviceIdForPublicKey(publicKey: Uint8Array): string {
  if (publicKey.length !== 32) throw new E2eError('Device public key must be 32 bytes');
  try { x25519.getSharedSecret(new Uint8Array(32).fill(0x42), publicKey); }
  catch { throw new E2eError('Device public key is noncontributory'); }
  return toHex(sha256(publicKey));
}

export function channelKeyId(cek: Uint8Array): string {
  if (cek.length !== 32) throw new E2eError('Channel key must be 32 bytes');
  const input = new Uint8Array(KEY_DOMAIN.length + cek.length);
  input.set(KEY_DOMAIN);
  input.set(cek, KEY_DOMAIN.length);
  return toHex(sha256(input));
}

function requireMatchingKey(cek: Uint8Array, context: EpochContext): void {
  requireEpochContext(context);
  if (channelKeyId(cek) !== context.keyId) throw new E2eError('Channel key fingerprint does not match the envelope');
}

function wrapFields(context: WrapContext, cek: Uint8Array, purpose: WrapPurpose): unknown[] {
  return [purpose === 'legacy' ? 'annex-legacy-key-wrap-v2' : 'annex-key-wrap-v2',
    context.authorityServer, context.channelId, context.epoch, context.keyId,
    context.recipientPseudonym, context.recipientDeviceId, toBase64(cek)];
}

/** A legacy bridge uses a separate domain and can never become a live wrap. */
export function sealChannelKeyV2(
  cek: Uint8Array, recipientPublicKey: Uint8Array, context: WrapContext, purpose: WrapPurpose = 'current',
): string {
  requireWrapContext(context);
  requireMatchingKey(cek, context);
  if (deviceIdForPublicKey(recipientPublicKey) !== context.recipientDeviceId) {
    throw new E2eError('Recipient device does not match its public key');
  }
  const sealed = toBase64(sealTo(utf8.encode(JSON.stringify(wrapFields(context, cek, purpose))), recipientPublicKey));
  if (sealed.length > 4096) throw new E2eError('Wrapped key exceeds the protocol limit');
  return sealed;
}

export function openChannelKeyV2(
  sealed: string, recipientSecret: Uint8Array, context: WrapContext, purpose: WrapPurpose = 'current',
): Uint8Array {
  requireWrapContext(context);
  if (deviceIdForPublicKey(publicKeyFromSecret(recipientSecret)) !== context.recipientDeviceId) {
    throw new E2eError('Wrapped key targets another device');
  }
  const bytes = decodeCanonicalBase64(sealed, 3072);
  if (bytes.length < 60) throw new E2eError('Sealed key envelope is truncated');
  const text = decodeUtf8Strict(openFrom(bytes, recipientSecret));
  let fields: unknown;
  try { fields = JSON.parse(text); }
  catch { throw new E2eError('Wrapped key has an invalid context envelope'); }
  if (!Array.isArray(fields) || fields.length !== 8 || typeof fields[7] !== 'string') {
    throw new E2eError('Wrapped key has an invalid context envelope');
  }
  const cek = decodeCanonicalBase64(fields[7], 32);
  requireMatchingKey(cek, context);
  if (text !== JSON.stringify(wrapFields(context, cek, purpose))) {
    throw new E2eError('Wrapped key context does not match this channel, epoch, or recipient');
  }
  return cek;
}

export function messageAadV2(context: EpochContext): Uint8Array {
  requireEpochContext(context);
  return utf8.encode(JSON.stringify(['annex-message-v2', context.authorityServer,
    context.channelId, context.epoch, context.keyId]));
}

export interface ParsedMessageV2 { epoch: number; keyId: string; ciphertext: Uint8Array }

export function parseMessageV2(body: string): ParsedMessageV2 {
  const match = /^e2e2:([1-9][0-9]*):([0-9a-f]{64}):([A-Za-z0-9+/=]+)$/.exec(body);
  if (!match) throw new E2eError('Invalid encrypted message envelope');
  const epoch = requireSafeInteger(Number(match[1]), 1);
  const ciphertext = decodeCanonicalBase64(match[3]);
  if (ciphertext.length < 28) throw new E2eError('Encrypted message is truncated');
  return { epoch, keyId: match[2], ciphertext };
}

export function encryptMessageV2(plaintext: string, cek: Uint8Array, context: EpochContext): string {
  requireMatchingKey(cek, context);
  // Do not silently replace malformed JavaScript strings before encryption.
  if (decodeUtf8Strict(utf8.encode(plaintext)) !== plaintext) throw new E2eError('Message is not valid Unicode');
  return `${E2E_V2_PREFIX}${context.epoch}:${context.keyId}:${toBase64(encryptContent(cek, utf8.encode(plaintext), messageAadV2(context)))}`;
}

export function decryptMessageV2(body: string, cek: Uint8Array, context: EpochContext): string {
  requireMatchingKey(cek, context);
  const parsed = parseMessageV2(body);
  if (parsed.epoch !== context.epoch || parsed.keyId !== context.keyId) throw new E2eError('Message uses another key epoch');
  return decodeUtf8Strict(decryptContent(cek, parsed.ciphertext, messageAadV2(context)));
}

export function attachmentAad(context: EpochContext, attachmentId: string): Uint8Array {
  requireEpochContext(context);
  if (!/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(attachmentId)) {
    throw new E2eError('Attachment ID must be a canonical UUID');
  }
  return utf8.encode(JSON.stringify(['annex-attachment-v1', context.authorityServer,
    context.channelId, attachmentId, context.epoch, context.keyId]));
}

export function encryptAttachment(
  plaintext: Uint8Array, fileKey: Uint8Array, context: EpochContext, attachmentId: string,
): Uint8Array {
  return encryptContent(fileKey, plaintext, attachmentAad(context, attachmentId));
}

export function decryptAttachment(
  ciphertext: Uint8Array, fileKey: Uint8Array, context: EpochContext, attachmentId: string,
  expectedHash: string, expectedBytes: number,
): Uint8Array {
  decodeHex32(expectedHash);
  requireSafeInteger(expectedBytes);
  if (ciphertext.length !== expectedBytes + 28 || toHex(sha256(ciphertext)) !== expectedHash) {
    throw new E2eError('Encrypted attachment size or digest does not match its authenticated descriptor');
  }
  return decryptContent(fileKey, ciphertext, attachmentAad(context, attachmentId));
}
