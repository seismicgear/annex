import { describe, expect, it, vi } from 'vitest';
import jsQR from 'jsqr';
import {
  decryptIdentity,
  encryptIdentity,
  generateQrSvg,
  encodePayload,
  decodePayload,
} from './device-link';
import type { StoredIdentity } from '@/types';
import type { DeviceLinkPayload } from '@/types';

const COMPLETE: StoredIdentity = {
  id: 'local-1',
  sk: '0a1b2c',
  roleCode: 1,
  nodeId: 7,
  commitmentHex: '0xdeadbeef',
  pseudonymId: 'p-1',
  sessionToken: null,
  serverSlug: 'alpha',
  leafIndex: 3,
  createdAt: '2026-01-01T00:00:00Z',
};

/** Independent old-format encoder to exercise backward compatibility and bad input. */
async function legacyTransfer(value: unknown): Promise<DeviceLinkPayload> {
  const salt = crypto.getRandomValues(new Uint8Array(16));
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const material = await crypto.subtle.importKey('raw', new TextEncoder().encode('123456'), 'PBKDF2', false, ['deriveKey']);
  const key = await crypto.subtle.deriveKey({ name: 'PBKDF2', salt, iterations: 100_000, hash: 'SHA-256' }, material, { name: 'AES-GCM', length: 256 }, false, ['encrypt']);
  const ciphertext = await crypto.subtle.encrypt({ name: 'AES-GCM', iv }, key, new TextEncoder().encode(JSON.stringify(value)));
  const b64 = (bytes: Uint8Array) => btoa(String.fromCharCode(...bytes));
  return { v: 1, salt: b64(salt), iv: b64(iv), data: b64(new Uint8Array(ciphertext)) };
}

/** Rasterize the emitted SVG squares; decoding uses an independent QR reader. */
function readQrSvg(svg: string): string | undefined {
  const element = new DOMParser().parseFromString(svg, 'image/svg+xml').documentElement;
  const width = Number(element.getAttribute('width'));
  const pixels = new Uint8ClampedArray(width * width * 4).fill(255);
  const path = element.querySelector('path')?.getAttribute('d') ?? '';
  for (const square of path.matchAll(/M([\d.]+) ([\d.]+)h([\d.]+)v([\d.]+)h-[\d.]+z/g)) {
    const [, rawX, rawY, rawW, rawH] = square;
    const [x, y, w, h] = [rawX, rawY, rawW, rawH].map(Number);
    for (let py = Math.max(0, Math.ceil(y - 0.5)); py < Math.min(width, y + h - 0.5); py++) {
      for (let px = Math.max(0, Math.ceil(x - 0.5)); px < Math.min(width, x + w - 0.5); px++) {
        const offset = (py * width + px) * 4;
        pixels[offset] = pixels[offset + 1] = pixels[offset + 2] = 0;
      }
    }
  }
  return jsQR(pixels, width, width, { inversionAttempts: 'dontInvert' })?.data;
}

describe('device link import', () => {
  it('reports a wrong pairing code after real AES-GCM authentication rejects it', async () => {
    const payload = await encryptIdentity(COMPLETE, '123456');
    await expect(decryptIdentity(payload, '654321')).rejects.toThrow(/check the pairing code and transfer code/i);
  });

  it('turns an empty browser OperationError into an actionable message', async () => {
    const payload = await encryptIdentity(COMPLETE, '123456');
    const decrypt = vi.spyOn(crypto.subtle, 'decrypt').mockRejectedValueOnce(new DOMException('', 'OperationError'));
    try {
      await expect(decryptIdentity(payload, '654321')).rejects.toThrow(/check the pairing code and transfer code/i);
    } finally {
      decrypt.mockRestore();
    }
  });

  it('round-trips a complete identity', async () => {
    const payload = await encryptIdentity(COMPLETE, '123456');
    const back = await decryptIdentity(payload, '123456');
    expect(back.sk).toBe(COMPLETE.sk);
    expect(back.commitmentHex).toBe(COMPLETE.commitmentHex);
  });

  it('refuses a transfer that decrypts but is not an identity', async () => {
    // AES-GCM authenticates the bytes, so this cannot come from a wrong
    // pairing code — it is what a payload from a build with a different
    // identity record, or one truncated before encryption, looks like. It
    // used to import cleanly and the dialog reported success over a record
    // the app could not sign with.
    const withoutKey: Record<string, unknown> = { ...COMPLETE };
    delete withoutKey.sk;
    delete withoutKey.roleCode;
    const payload = await legacyTransfer(withoutKey);

    await expect(decryptIdentity(payload, '123456')).rejects.toThrow(/missing identity fields/i);
  });

  it('refuses a transfer whose payload is not an object at all', async () => {
    const payload = await legacyTransfer('not-an-identity');
    await expect(decryptIdentity(payload, '123456')).rejects.toThrow(/did not contain an identity/i);
  });

  it('accepts legacy whole-record transfers but discards device-local credentials', async () => {
    const old = { ...COMPLETE, sessionToken: 'old-token', zkProofPayload: 'cached-proof', lastUsedAt: 'old-device-use' };
    const imported = await decryptIdentity(await legacyTransfer(old), '123456');
    expect(imported).toMatchObject({ sk: old.sk, roleCode: old.roleCode, nodeId: old.nodeId, commitmentHex: old.commitmentHex, pseudonymId: old.pseudonymId, serverSlug: old.serverSlug, createdAt: old.createdAt, sessionToken: null, leafIndex: null });
    expect(imported.id).not.toBe(old.id);
    expect(imported.zkProofPayload).toBeUndefined();
    expect(imported.lastUsedAt).toBeUndefined();
  });

  it('transfers an identity created offline before choosing a server', async () => {
    const identity = { ...COMPLETE, serverSlug: '', pseudonymId: null, leafIndex: null };
    const imported = await decryptIdentity(await encryptIdentity(identity, '123456'), '123456');
    expect(imported).toMatchObject({ sk: identity.sk, commitmentHex: identity.commitmentHex, serverSlug: '', pseudonymId: null });
  });
});

describe('generateQrSvg', () => {
  it('can be scanned by an independent reader and decrypted to the original identity', async () => {
    const encrypted = await encryptIdentity(COMPLETE, '123456');
    const encoded = encodePayload(encrypted);
    const scanned = readQrSvg(generateQrSvg(encoded));
    expect(scanned).toBe(encoded);
    expect(await decryptIdentity(decodePayload(scanned!), '123456')).toMatchObject({
      sk: COMPLETE.sk, commitmentHex: COMPLETE.commitmentHex, roleCode: COMPLETE.roleCode,
      nodeId: COMPLETE.nodeId, pseudonymId: COMPLETE.pseudonymId, serverSlug: COMPLETE.serverSlug,
    });
  });

  it('independently scans a full registered identity at the displayed size despite large source caches', async () => {
    const registered: StoredIdentity = {
      id: '9e82f844-7fd8-4c76-a1ac-066c3dc9a7ab',
      sk: 'a'.repeat(64), roleCode: 1, nodeId: 482913,
      commitmentHex: 'b'.repeat(64), pseudonymId: 'c'.repeat(64),
      serverSlug: 'http-127-0-0-1-3000', leafIndex: 1537,
      createdAt: '2026-10-12T22:03:00.000Z', lastUsedAt: '2026-10-12T22:05:00.000Z',
      sessionToken: 'registered-session.'.repeat(20),
      zkProofPayload: JSON.stringify({ protocolVersion: 2, proof: { pi_a: ['1'.repeat(77), '2'.repeat(77), '1'], pi_b: [['3'.repeat(77), '4'.repeat(77)], ['5'.repeat(77), '6'.repeat(77)], ['1', '0']], pi_c: ['7'.repeat(77), '8'.repeat(77), '1'] }, publicSignals: Array.from({ length: 64 }, () => '9'.repeat(77)) }),
    };
    const encrypted = await encryptIdentity(registered, '123456');
    const encoded = encodePayload(encrypted);
    const scanned = readQrSvg(generateQrSvg(encoded));
    expect(scanned === encoded, 'A complete registered identity must fit and scan at the normal rendered size').toBe(true);
    const restored = await decryptIdentity(decodePayload(scanned!), '123456');
    expect(restored).toMatchObject({ sk: registered.sk, roleCode: registered.roleCode, nodeId: registered.nodeId, commitmentHex: registered.commitmentHex, pseudonymId: registered.pseudonymId, serverSlug: registered.serverSlug, createdAt: registered.createdAt, sessionToken: null, leafIndex: null });
    expect(restored.zkProofPayload).toBeUndefined();
    expect(restored.lastUsedAt).toBeUndefined();
  });

  it('preserves Unicode text when scanned', () => {
    const text = 'Annex — Boise 📱';
    expect(readQrSvg(generateQrSvg(text))).toBe(text);
  });

  it('rejects payloads that exceed QR capacity instead of drawing an unreadable image', () => {
    expect(() => generateQrSvg('x'.repeat(10_000))).toThrow(/too large.*copy the transfer code/i);
  });

  it('rejects non-finite image dimensions', () => {
    expect(() => generateQrSvg('payload', Number.NaN)).toThrow(/size must be positive/i);
  });

  it('emits one path rather than a node per module', () => {
    // The per-module <rect> version produced thousands of DOM nodes for a
    // graphic. That was slow to lay out and slow for anything
    // walking the DOM — it timed out Playwright's trace snapshotter outright.
    const svg = generateQrSvg('some-transfer-payload-that-is-reasonably-long');

    const rects = svg.match(/<rect/g) ?? [];
    const paths = svg.match(/<path/g) ?? [];

    expect(rects, 'only the white background should be a rect').toHaveLength(1);
    expect(paths).toHaveLength(1);
  });

  it('produces a well-formed, correctly sized svg', () => {
    const svg = generateQrSvg('payload', 128);
    expect(svg.startsWith('<svg')).toBe(true);
    expect(svg.endsWith('</svg>')).toBe(true);
    expect(svg).toContain('viewBox="0 0 128 128"');
    expect(svg).toContain('width="128"');
  });

  it('is deterministic for the same payload', () => {
    expect(generateQrSvg('abc')).toBe(generateQrSvg('abc'));
  });

  it('differs for different payloads', () => {
    expect(generateQrSvg('abc')).not.toBe(generateQrSvg('abd'));
  });
});
