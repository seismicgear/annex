/**
 * End-to-end encryption primitives for opt-in content-blind channels.
 *
 * This is the client counterpart of `crates/annex-federation/src/seal.rs`.
 * The sealed-box construction is **byte-identical** across the two languages
 * and pinned by a cross-language Known-Answer-Test (`e2e.test.ts` here and
 * `seal::tests::x25519_kat_is_stable_and_cross_language` in Rust), so a member
 * key wrapped in Rust opens in TypeScript and vice-versa. That is what lets an
 * AI agent (which may run inside the Rust server) and a human (in this browser
 * client) share the same channel key without the server ever seeing it.
 *
 * Two layers:
 *   1. `sealTo` / `openFrom` — anonymous sealed box to a recipient's X25519
 *      public key. Used to WRAP a per-channel content key (CEK) to each member.
 *   2. `encryptContent` / `decryptContent` — symmetric AEAD with the CEK. Used
 *      to encrypt the actual message bodies. The server stores only ciphertext.
 *
 * Construction (sealed box):
 *   epk        = X25519 public of a fresh ephemeral secret (forward secrecy)
 *   shared     = X25519(ephemeral_secret, recipient_pub)
 *   key        = HKDF-SHA256(ikm = shared, salt = epk‖recipient_pub,
 *                            info = "annex-e2e-seal-v1")  -> 32 bytes
 *   ciphertext = ChaCha20-Poly1305(key, nonce, aad = epk).encrypt(plaintext)
 *   wire       = epk(32) ‖ nonce(12) ‖ ciphertext+tag
 */

import { x25519 } from '@noble/curves/ed25519.js';
import { chacha20poly1305 } from '@noble/ciphers/chacha.js';
import { hkdf } from '@noble/hashes/hkdf.js';
import { sha256 } from '@noble/hashes/sha2.js';

/** HKDF domain-separation label. MUST equal `seal::E2E_INFO` in Rust. */
const E2E_INFO = new TextEncoder().encode('annex-e2e-seal-v1');
const EPK_LEN = 32;
const NONCE_LEN = 12;
const KEY_LEN = 32;

export class E2eError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'E2eError';
  }
}

function randomBytes(n: number): Uint8Array {
  const b = new Uint8Array(n);
  crypto.getRandomValues(b);
  return b;
}

/** Fresh 32-byte X25519 device secret. Persist locally; never send it anywhere. */
export function generateDeviceSecret(): Uint8Array {
  // Any 32 random bytes are a valid X25519 secret (clamped on use by RFC 7748).
  return randomBytes(32);
}

/** Fresh 32-byte channel content key (CEK). */
export function generateChannelKey(): Uint8Array {
  return randomBytes(KEY_LEN);
}

/** The X25519 public key advertised in the member key directory. */
export function publicKeyFromSecret(secret: Uint8Array): Uint8Array {
  if (secret.length !== 32) throw new E2eError('device secret must be 32 bytes');
  return x25519.getPublicKey(secret);
}

function deriveKey(shared: Uint8Array, epk: Uint8Array, recipientPub: Uint8Array): Uint8Array {
  const salt = new Uint8Array(EPK_LEN + 32);
  salt.set(epk, 0);
  salt.set(recipientPub, EPK_LEN);
  return hkdf(sha256, shared, salt, E2E_INFO, KEY_LEN);
}

/** Internal core; ephemeral secret + nonce injected so the KAT can be deterministic. */
function sealToWith(
  plaintext: Uint8Array,
  recipientPub: Uint8Array,
  ephemeralSecret: Uint8Array,
  nonce: Uint8Array,
): Uint8Array {
  if (recipientPub.length !== 32) throw new E2eError('recipient public key must be 32 bytes');
  const epk = x25519.getPublicKey(ephemeralSecret);
  const shared = x25519.getSharedSecret(ephemeralSecret, recipientPub);
  const key = deriveKey(shared, epk, recipientPub);
  const ct = chacha20poly1305(key, nonce, epk).encrypt(plaintext);
  const out = new Uint8Array(EPK_LEN + NONCE_LEN + ct.length);
  out.set(epk, 0);
  out.set(nonce, EPK_LEN);
  out.set(ct, EPK_LEN + NONCE_LEN);
  return out;
}

/**
 * Seal `plaintext` so only the holder of `recipientPub`'s secret can open it.
 * Safe to hand to the (untrusted, content-blind) server. Byte-compatible with
 * Rust `seal::seal_x25519`.
 */
export function sealTo(plaintext: Uint8Array, recipientPub: Uint8Array): Uint8Array {
  return sealToWith(plaintext, recipientPub, generateDeviceSecret(), randomBytes(NONCE_LEN));
}

/** Open a blob produced by `sealTo` (or Rust `seal_x25519`). */
export function openFrom(blob: Uint8Array, recipientSecret: Uint8Array): Uint8Array {
  if (blob.length < EPK_LEN + NONCE_LEN) throw new E2eError('sealed blob too short');
  const epk = blob.subarray(0, EPK_LEN);
  const nonce = blob.subarray(EPK_LEN, EPK_LEN + NONCE_LEN);
  const ct = blob.subarray(EPK_LEN + NONCE_LEN);
  const recipientPub = x25519.getPublicKey(recipientSecret);
  const shared = x25519.getSharedSecret(recipientSecret, epk);
  const key = deriveKey(shared, epk, recipientPub);
  try {
    return chacha20poly1305(key, nonce, epk).decrypt(ct);
  } catch {
    throw new E2eError('decryption failed (wrong key or tampered ciphertext)');
  }
}

/**
 * Encrypt a message body with the channel content key (CEK). `aad` optionally
 * binds the ciphertext to a context (e.g. channel id) so it cannot be replayed
 * into a different channel that happens to share the key. Wire: nonce(12)‖ct.
 */
export function encryptContent(cek: Uint8Array, plaintext: Uint8Array, aad?: Uint8Array): Uint8Array {
  if (cek.length !== KEY_LEN) throw new E2eError('channel key must be 32 bytes');
  const nonce = randomBytes(NONCE_LEN);
  const ct = chacha20poly1305(cek, nonce, aad).encrypt(plaintext);
  const out = new Uint8Array(NONCE_LEN + ct.length);
  out.set(nonce, 0);
  out.set(ct, NONCE_LEN);
  return out;
}

/** Decrypt a body produced by `encryptContent`. */
export function decryptContent(cek: Uint8Array, blob: Uint8Array, aad?: Uint8Array): Uint8Array {
  if (cek.length !== KEY_LEN) throw new E2eError('channel key must be 32 bytes');
  if (blob.length < NONCE_LEN) throw new E2eError('content blob too short');
  const nonce = blob.subarray(0, NONCE_LEN);
  const ct = blob.subarray(NONCE_LEN);
  try {
    return chacha20poly1305(cek, nonce, aad).decrypt(ct);
  } catch {
    throw new E2eError('content decryption failed (wrong key or tampered ciphertext)');
  }
}

// ---- encoding helpers (channel ciphertext travels as base64 text) ----

export function toBase64(bytes: Uint8Array): string {
  let bin = '';
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin);
}

export function fromBase64(s: string): Uint8Array {
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

export function toHex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

export function fromHex(hex: string): Uint8Array {
  const clean = hex.startsWith('0x') ? hex.slice(2) : hex;
  if (clean.length % 2 !== 0) throw new E2eError('odd-length hex');
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(clean.substr(i * 2, 2), 16);
  return out;
}

export const utf8 = {
  encode: (s: string): Uint8Array => new TextEncoder().encode(s),
  decode: (b: Uint8Array): string => new TextDecoder().decode(b),
};

// Exposed for the cross-language KAT only.
export const __test__ = { sealToWith };
