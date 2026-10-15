/**
 * Device linking — encrypts and packages identity data for QR-based transfer.
 *
 * Protocol:
 * 1. Source device generates a random 6-digit pairing code shown to the user.
 * 2. A 256-bit AES-GCM key is derived from the code via PBKDF2.
 * 3. The identity bundle is encrypted and encoded as a compact JSON payload.
 * 4. The payload is serialized into a QR-compatible string.
 * 5. Target device scans QR, user enters the same pairing code, decrypts.
 */

import type { StoredIdentity, DeviceLinkPayload } from '@/types';
import qrcode from 'qrcode-generator';

qrcode.stringToBytes = (value: string) => Array.from(new TextEncoder().encode(value));

const PBKDF2_ITERATIONS = 100_000;

/** Portable identity material; proof/session caches belong to the source device. */
type PortableIdentity = Pick<StoredIdentity,
  'sk' | 'roleCode' | 'nodeId' | 'commitmentHex' | 'pseudonymId' | 'serverSlug' | 'createdAt'>;

function portableIdentity(identity: PortableIdentity): PortableIdentity {
  return {
    sk: identity.sk,
    roleCode: identity.roleCode,
    nodeId: identity.nodeId,
    commitmentHex: identity.commitmentHex,
    pseudonymId: identity.pseudonymId ?? null,
    serverSlug: identity.serverSlug,
    createdAt: identity.createdAt,
  };
}

/** Generate a random 6-digit numeric pairing code. */
export function generatePairingCode(): string {
  const arr = new Uint32Array(1);
  crypto.getRandomValues(arr);
  return String(arr[0] % 1_000_000).padStart(6, '0');
}

async function deriveKey(code: string, salt: Uint8Array): Promise<CryptoKey> {
  const enc = new TextEncoder();
  const keyMaterial = await crypto.subtle.importKey(
    'raw',
    enc.encode(code),
    'PBKDF2',
    false,
    ['deriveKey'],
  );
  return crypto.subtle.deriveKey(
    { name: 'PBKDF2', salt: salt.buffer as ArrayBuffer, iterations: PBKDF2_ITERATIONS, hash: 'SHA-256' },
    keyMaterial,
    { name: 'AES-GCM', length: 256 },
    false,
    ['encrypt', 'decrypt'],
  );
}

function toBase64(buf: ArrayBuffer | Uint8Array): string {
  const bytes = buf instanceof Uint8Array ? buf : new Uint8Array(buf);
  let binary = '';
  for (const b of bytes) binary += String.fromCharCode(b);
  return btoa(binary);
}

function fromBase64(b64: string): Uint8Array {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

/**
 * Encrypt an identity for transfer. Returns the QR payload and the pairing code.
 * The pairing code must be communicated out-of-band (displayed on screen).
 */
export async function encryptIdentity(
  identity: StoredIdentity,
  pairingCode: string,
): Promise<DeviceLinkPayload> {
  assertPortableIdentity(identity);
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const key = await deriveKey(pairingCode, salt);

  // Explicit allow-list: large cached proofs made a fully registered identity
  // exceed QR capacity, and a new device must establish its own session.
  const plaintext = new TextEncoder().encode(JSON.stringify(portableIdentity(identity)));
  const ciphertext = await crypto.subtle.encrypt(
    { name: 'AES-GCM', iv: iv.buffer as ArrayBuffer },
    key,
    plaintext,
  );

  return {
    v: 1,
    data: toBase64(new Uint8Array(ciphertext)),
    iv: toBase64(iv),
    salt: toBase64(salt),
  };
}

/**
 * Decrypt an identity from a scanned QR payload using the pairing code.
 * Throws if the code is wrong or data is corrupted.
 */
export async function decryptIdentity(
  payload: DeviceLinkPayload,
  pairingCode: string,
): Promise<StoredIdentity> {
  const salt = fromBase64(payload.salt);
  const iv = fromBase64(payload.iv);
  const ciphertext = fromBase64(payload.data);
  const key = await deriveKey(pairingCode, salt);

  let plaintext: ArrayBuffer;
  try {
    plaintext = await crypto.subtle.decrypt(
      { name: 'AES-GCM', iv: iv.buffer as ArrayBuffer },
      key,
      ciphertext.buffer as ArrayBuffer,
    );
  } catch {
    // Chromium may return OperationError with an empty message. A failed
    // authentication must still produce a visible, actionable import error.
    throw new Error('Unable to decrypt this transfer. Check the pairing code and transfer code, then try again.');
  }

  const parsed: unknown = JSON.parse(new TextDecoder().decode(plaintext));
  assertPortableIdentity(parsed);
  // Accept legacy v1 transfers containing the entire StoredIdentity, but only
  // import portable fields. Never resurrect stale bearer tokens/proof caches.
  return {
    ...portableIdentity(parsed),
    id: crypto.randomUUID(),
    createdAt: parsed.createdAt || new Date().toISOString(),
    sessionToken: null,
    leafIndex: null,
  };
}

/**
 * Narrow a decrypted blob to a `StoredIdentity`, or throw.
 *
 * AES-GCM authenticates the ciphertext, so a wrong pairing code cannot get
 * this far — but authentication says the bytes are the ones that were sent,
 * not that they describe a usable identity. `as StoredIdentity` asserted the
 * shape without checking it, so a payload from a build whose identity record
 * differed, or one truncated before encryption, imported cleanly and the
 * dialog said "Identity imported successfully!" over a record the app could
 * not sign with. The failure surfaced later, somewhere else, as an identity
 * that simply did not work.
 */
function assertPortableIdentity(value: unknown): asserts value is PortableIdentity {
  const v = value as Record<string, unknown> | null;
  if (!v || typeof v !== 'object') {
    throw new Error('Transfer did not contain an identity');
  }
  const missing: string[] = (['sk', 'commitmentHex'] as const).filter(
    (k) => typeof v[k] !== 'string' || !(v[k] as string),
  );
  // A locally created identity legitimately has no server selected yet.
  if (typeof v.serverSlug !== 'string') missing.push('serverSlug');
  if (!Number.isSafeInteger(v.roleCode)) missing.push('roleCode');
  if (!Number.isSafeInteger(v.nodeId)) missing.push('nodeId');
  if (v.pseudonymId != null && typeof v.pseudonymId !== 'string') missing.push('pseudonymId');
  if (v.createdAt != null && typeof v.createdAt !== 'string') missing.push('createdAt');
  if (missing.length > 0) {
    throw new Error(`Transfer is missing identity fields: ${missing.join(', ')}`);
  }
}

/** Encode a DeviceLinkPayload as a compact string suitable for QR code. */
export function encodePayload(payload: DeviceLinkPayload): string {
  return JSON.stringify(payload);
}

/** Decode a scanned QR string back into a DeviceLinkPayload. */
export function decodePayload(raw: string): DeviceLinkPayload {
  const parsed = JSON.parse(raw);
  if (parsed.v !== 1 || !parsed.data || !parsed.iv || !parsed.salt) {
    throw new Error('Invalid device link payload');
  }
  return parsed as DeviceLinkPayload;
}

/**
 * Generate an SVG QR code from a string.
 * Uses QR byte encoding with medium error correction and a four-module quiet
 * zone, so standard QR readers can recover the encrypted transfer payload.
 * Throws if the payload exceeds QR capacity or the requested size is invalid.
 */
export function generateQrSvg(data: string, size = 384): string {
  if (!Number.isFinite(size) || size <= 0) throw new Error('QR image size must be positive');
  const code = qrcode(0, 'M');
  code.addData(data, 'Byte');
  try {
    code.make();
  } catch {
    throw new Error('This identity is too large for a QR code. Copy the transfer code instead.');
  }
  const moduleCount = code.getModuleCount();
  const quietZone = 4;
  // All SVG attribute values below are derived from numeric computations
  // (no user-controlled strings), so XSS injection via attribute values is
  // not possible. The `size` and coordinates are always finite numbers.
  const moduleSize = size / (moduleCount + 2 * quietZone);

  // Every dark module is emitted into ONE path rather than its own <rect>.
  // The per-rect version produced thousands of DOM nodes, which was slow to
  // lay out and slow for anything that walks the
  // DOM — it timed out Playwright's trace snapshotter outright. A single
  // path renders identically at a fraction of the node count.
  const segments: string[] = [];
  for (let row = 0; row < moduleCount; row++) {
    for (let col = 0; col < moduleCount; col++) {
      if (code.isDark(row, col)) {
        const x = (col + quietZone) * moduleSize;
        const y = (row + quietZone) * moduleSize;
        // `h`/`v`/`h`/`z` draws the square relative to the `M` origin.
        segments.push(`M${x} ${y}h${moduleSize}v${moduleSize}h-${moduleSize}z`);
      }
    }
  }

  const parts: string[] = [
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${size} ${size}" width="${size}" height="${size}" shape-rendering="crispEdges">`,
    `<rect width="${size}" height="${size}" fill="white"/>`,
  ];
  if (segments.length > 0) {
    parts.push(`<path d="${segments.join('')}" fill="black"/>`);
  }

  parts.push('</svg>');
  return parts.join('');
}
