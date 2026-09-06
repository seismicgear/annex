import { describe, expect, it } from 'vitest';
import {
  decryptIdentity,
  encryptIdentity,
  generateQrSvg,
} from './device-link';
import type { StoredIdentity } from '@/types';

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
} as StoredIdentity;

describe('device link import', () => {
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
    const payload = await encryptIdentity(withoutKey as never, '123456');

    await expect(decryptIdentity(payload, '123456')).rejects.toThrow(/missing identity fields/i);
  });

  it('refuses a transfer whose payload is not an object at all', async () => {
    const payload = await encryptIdentity('not-an-identity' as never, '123456');
    await expect(decryptIdentity(payload, '123456')).rejects.toThrow(/did not contain an identity/i);
  });
});

describe('generateQrSvg', () => {
  it('emits one path rather than a node per module', () => {
    // The per-module <rect> version produced thousands of DOM nodes for a
    // decorative graphic. That was slow to lay out and slow for anything
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
