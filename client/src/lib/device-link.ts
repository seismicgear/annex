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

const PBKDF2_ITERATIONS = 100_000;

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
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const key = await deriveKey(pairingCode, salt);

  const plaintext = new TextEncoder().encode(JSON.stringify(identity));
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

  const plaintext = await crypto.subtle.decrypt(
    { name: 'AES-GCM', iv: iv.buffer as ArrayBuffer },
    key,
    ciphertext.buffer as ArrayBuffer,
  );

  return JSON.parse(new TextDecoder().decode(plaintext)) as StoredIdentity;
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
 * Uses a simple implementation that works without external dependencies.
 * Returns an SVG string.
 */
export function generateQrSvg(data: string, size = 256): string {
  // Encode data into a binary matrix for a visual data representation.
  // NOTE: This is NOT a standards-compliant QR code and cannot be scanned
  // by QR reader apps. It is a visual encoding used alongside the paste-based
  // import flow. The "receive" tab allows manual paste of the encoded payload.
  const modules = encodeToMatrix(data);
  const moduleCount = modules.length;
  // All SVG attribute values below are derived from numeric computations
  // (no user-controlled strings), so XSS injection via attribute values is
  // not possible. The `size` and coordinates are always finite numbers.
  const moduleSize = size / moduleCount;

  const parts: string[] = [
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${size} ${size}" width="${size}" height="${size}">`,
    `<rect width="${size}" height="${size}" fill="white"/>`,
  ];

  for (let row = 0; row < moduleCount; row++) {
    for (let col = 0; col < moduleCount; col++) {
      if (modules[row][col]) {
        const x = col * moduleSize;
        const y = row * moduleSize;
        parts.push(`<rect x="${x}" y="${y}" width="${moduleSize}" height="${moduleSize}" fill="black"/>`);
      }
    }
  }

  parts.push('</svg>');
  return parts.join('');
}

/**
 * Encode a string into a boolean matrix for QR-like display.
 * This produces a deterministic visual pattern from the data.
 */
function encodeToMatrix(data: string): boolean[][] {
  // Use a grid size based on data length. Each character maps to ~8 bits.
  const bits: boolean[] = [];
  for (let i = 0; i < data.length; i++) {
    const code = data.charCodeAt(i);
    for (let b = 7; b >= 0; b--) {
      bits.push(((code >> b) & 1) === 1);
    }
  }

  // Calculate grid size (square root of bits count, rounded up, min 21 for QR-like look)
  const sideLen = Math.max(21, Math.ceil(Math.sqrt(bits.length + 64)));
  const matrix: boolean[][] = Array.from({ length: sideLen }, () =>
    Array(sideLen).fill(false),
  );

  // Add finder patterns (top-left, top-right, bottom-left) for QR-like appearance
  const addFinder = (startRow: number, startCol: number) => {
    for (let r = 0; r < 7; r++) {
      for (let c = 0; c < 7; c++) {
        const isOuter = r === 0 || r === 6 || c === 0 || c === 6;
        const isInner = r >= 2 && r <= 4 && c >= 2 && c <= 4;
        matrix[startRow + r][startCol + c] = isOuter || isInner;
      }
    }
  };

  addFinder(0, 0);
  addFinder(0, sideLen - 7);
  addFinder(sideLen - 7, 0);

  // Fill data bits into the matrix (skipping finder areas)
  let bitIdx = 0;
  for (let row = 0; row < sideLen; row++) {
    for (let col = 0; col < sideLen; col++) {
      // Skip finder pattern regions
      const inFinder =
        (row < 8 && col < 8) ||
        (row < 8 && col >= sideLen - 8) ||
        (row >= sideLen - 8 && col < 8);
      if (inFinder) continue;

      if (bitIdx < bits.length) {
        matrix[row][col] = bits[bitIdx];
        bitIdx++;
      }
    }
  }

  return matrix;
}
