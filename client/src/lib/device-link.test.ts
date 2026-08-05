import { describe, expect, it } from 'vitest';
import { generateQrSvg } from './device-link';

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
