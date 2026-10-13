// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { stripImageMetadata } from './image-privacy';

const corpus = new URL('../../../crates/annex-server/tests/fixtures/image-privacy/', import.meta.url);
const cases = [['progressive.jpg', 'image/jpeg'], ['animated.png', 'image/png'], ['animated.gif', 'image/gif'],
  ['animated.webp', 'image/webp'], ['still.webp', 'image/webp']] as const;
const bytes = (name: string) => new Uint8Array(readFileSync(new URL(name, corpus)));
describe('lossless image privacy containers', () => {
  it.each(cases)('removes private metadata throughout %s without a second mutation', (name, mime) => {
    const original = bytes(name);
    const cleaned = stripImageMetadata(original, mime);
    expect(new TextDecoder().decode(original)).toContain('ANNEX_PRIVATE_METADATA_LOCATION_AUTHOR_12345');
    expect(new TextDecoder().decode(cleaned)).not.toContain('ANNEX_PRIVATE_METADATA_LOCATION_AUTHOR_12345');
    expect(cleaned.length).toBeLessThan(original.length);
    expect(stripImageMetadata(cleaned, mime)).toEqual(cleaned);
    // Optional receipt for independent Pillow/libwebp pixel, timing and loop
    // comparison. The parser never invokes that decoder or consumes its answer.
    if (process.env.ANNEX_IMAGE_EVIDENCE_DIR) {
      mkdirSync(process.env.ANNEX_IMAGE_EVIDENCE_DIR, { recursive: true });
      writeFileSync(join(process.env.ANNEX_IMAGE_EVIDENCE_DIR, name), cleaned);
    }
  });
  it.each(cases)('refuses truncated %s rather than uploading partial bytes', (name, mime) => {
    const original = bytes(name);
    for (const length of [1, 7, 12, Math.floor(original.length / 2), original.length - 1]) {
      expect(() => stripImageMetadata(original.subarray(0, length), mime)).toThrow(/malformed/);
    }
  });
  it('refuses GIF loop controls carrying unrecognized extra subblocks', () => {
    const original = bytes('animated.gif');
    const index = Buffer.from(original).indexOf('NETSCAPE2.0');
    expect(index).toBeGreaterThan(0);
    const end = index + 11 + 4;
    const injected = new Uint8Array(original.length + 3);
    injected.set(original.subarray(0, end));
    injected.set([2, 80, 81], end);
    injected.set(original.subarray(end), end + 3);
    expect(() => stripImageMetadata(injected, 'image/gif')).toThrow(/malformed/);
  });
  it('refuses empty image containers', () => {
    expect(() => stripImageMetadata(Uint8Array.of(255, 216, 255, 217), 'image/jpeg')).toThrow();
    const gif = bytes('animated.gif');
    const header = 13 + ((gif[10] & 128) ? 3 * 2 ** ((gif[10] & 7) + 1) : 0);
    expect(() => stripImageMetadata(Uint8Array.from([...gif.subarray(0, header), 59]), 'image/gif')).toThrow();
    expect(() => stripImageMetadata(new TextEncoder().encode('RIFF\u0004\0\0\0WEBP'), 'image/webp')).toThrow();
  });
});
