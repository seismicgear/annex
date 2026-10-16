import { ed25519, x25519 } from '@noble/curves/ed25519.js';
import { chacha20poly1305 } from '@noble/ciphers/chacha.js';
import { hkdf } from '@noble/hashes/hkdf.js';
import { sha256 } from '@noble/hashes/sha2.js';
import { bytesToHex, concatBytes, randomBytes, utf8ToBytes } from '@noble/hashes/utils.js';

const DOMAIN = utf8ToBytes('annex-content-blind-v2');
const CHUNK = 48 * 1024;
const MAX_FRAME = 8 * 1024 * 1024;
export interface SealedPacket { sequence: number; data: string }

export function toBase64(value: Uint8Array): string {
  let raw = ''; for (const byte of value) raw += String.fromCharCode(byte);
  return btoa(raw);
}
export function fromBase64(value: string): Uint8Array {
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(value)) throw new Error('Invalid tunnel encoding');
  const bytes = Uint8Array.from(atob(value), char => char.charCodeAt(0));
  if (toBase64(bytes) !== value) throw new Error('Noncanonical tunnel encoding');
  return bytes;
}

export function clientHello(secret = randomBytes(32), nonce = randomBytes(32)) {
  const hello = concatBytes(x25519.getPublicKey(secret), nonce);
  let consumed = false;
  return {
    hello: toBase64(hello),
    finish(hostId: string, welcome: string): TunnelCipher {
      if (consumed) throw new Error('Tunnel handshake already consumed');
      consumed = true;
      try {
        const bytes = fromBase64(welcome);
        if (bytes.length !== 128) throw new Error('Invalid host handshake');
        const identity = bytes.subarray(0, 32), ephemeral = bytes.subarray(32, 64);
        if (bytesToHex(sha256(identity)) !== hostId) throw new Error('The router returned a different server identity');
        const transcript = concatBytes(DOMAIN, identity, hello, ephemeral);
        if (!ed25519.verify(bytes.subarray(64), transcript, identity, { zip215: false })) throw new Error('The server did not authenticate this connection');
        return new TunnelCipher(x25519.getSharedSecret(secret, ephemeral), sha256(transcript), false);
      } finally { secret.fill(0); }
    },
    destroy() { consumed = true; secret.fill(0); },
  };
}

/** Endpoint-only cipher. The URL pins the signing key, which signs both
 * ephemeral keys and the client's fresh nonce. Each direction has its own key;
 * authenticated sequence numbers reject replay, reordering and reflection.
 */
export class TunnelCipher {
  private outbound: Uint8Array;
  private inbound: Uint8Array;
  private sent = 0;
  private received = 0;
  private fragments: Uint8Array[] = [];
  private size = 0;
  private destroyed = false;
  constructor(shared: Uint8Array, private transcript: Uint8Array, host: boolean) {
    const c2h = hkdf(sha256, shared, transcript, concatBytes(DOMAIN, utf8ToBytes('/client-to-host')), 32);
    const h2c = hkdf(sha256, shared, transcript, concatBytes(DOMAIN, utf8ToBytes('/host-to-client')), 32);
    shared.fill(0);
    this.outbound = host ? h2c : c2h; this.inbound = host ? c2h : h2c;
  }
  private nonce(sequence: number): Uint8Array {
    const nonce = new Uint8Array(12);
    new DataView(nonce.buffer).setUint32(8, sequence, false);
    return nonce;
  }
  seal(value: unknown): SealedPacket[] {
    if (this.destroyed) throw new Error('Tunnel is closed');
    const bytes = utf8ToBytes(JSON.stringify(value));
    if (bytes.length > MAX_FRAME) throw new Error('Tunnel frame exceeds its limit');
    const packets: SealedPacket[] = [];
    for (let offset = 0; offset < bytes.length; offset += CHUNK) {
      if (this.sent > 0xffffffff) { this.destroy(); throw new Error('Tunnel key exhausted'); }
      const chunk = bytes.subarray(offset, offset + CHUNK);
      const padded = new Uint8Array(Math.ceil((chunk.length + 5) / 256) * 256);
      padded[0] = offset + chunk.length === bytes.length ? 1 : 0;
      new DataView(padded.buffer).setUint32(1, chunk.length, false); padded.set(chunk, 5);
      const sequence = this.sent++, nonce = this.nonce(sequence);
      packets.push({ sequence, data: toBase64(chacha20poly1305(this.outbound, nonce, concatBytes(this.transcript, nonce)).encrypt(padded)) });
      padded.fill(0);
    }
    bytes.fill(0); return packets;
  }
  open(packet: SealedPacket): unknown | undefined {
    try {
      if (this.destroyed || packet.sequence !== this.received || this.received > 0xffffffff) throw new Error('Tunnel replay or missing packet');
      const sealed = fromBase64(packet.data);
      if (sealed.length < 272 || sealed.length > CHUNK + 272) throw new Error('Invalid tunnel packet length');
      const nonce = this.nonce(packet.sequence);
      const plain = chacha20poly1305(this.inbound, nonce, concatBytes(this.transcript, nonce)).decrypt(sealed);
      const length = new DataView(plain.buffer, plain.byteOffset, plain.byteLength).getUint32(1, false);
      if (plain[0] > 1 || length > CHUNK || length + 5 > plain.length || plain.length % 256
        || plain.subarray(length + 5).some(byte => byte !== 0)) throw new Error('Invalid tunnel framing');
      this.received++;
      this.size += length;
      if (this.size > MAX_FRAME) throw new Error('Tunnel reassembly exceeds its limit');
      this.fragments.push(plain.slice(5, length + 5));
      const last = plain[0] === 1; plain.fill(0);
      if (!last) return undefined;
      const body = concatBytes(...this.fragments);
      for (const fragment of this.fragments) fragment.fill(0);
      this.fragments = []; this.size = 0;
      try { return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(body)); }
      finally { body.fill(0); }
    } catch (error) { this.destroy(); throw error; }
  }
  destroy() {
    this.destroyed = true; this.outbound.fill(0); this.inbound.fill(0);
    for (const fragment of this.fragments) fragment.fill(0);
    this.fragments = []; this.size = 0;
  }
}
