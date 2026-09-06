import { describe, expect, it } from 'vitest';
import { describeServerUrlError, normalizeServerUrl } from './url';

describe('normalizeServerUrl', () => {
  it('defaults to http for loopback and private addresses', () => {
    expect(normalizeServerUrl('localhost:3000')).toBe('http://localhost:3000');
    expect(normalizeServerUrl('192.168.1.4')).toBe('http://192.168.1.4');
  });

  it('defaults to https for public hostnames', () => {
    expect(normalizeServerUrl('annex.example.com')).toBe('https://annex.example.com');
  });

  it('preserves an explicit scheme', () => {
    expect(normalizeServerUrl('http://annex.example.com')).toBe('http://annex.example.com');
  });

  it('refuses a non-HTTP scheme by name', () => {
    expect(() => normalizeServerUrl('ftp://files.example.com')).toThrow(
      /only http and https/i,
    );
  });
});

/**
 * The wording two dialogs share.
 *
 * `StartupModeSelector` (first run) and `ServerHub`'s Join-a-Server dialog ask
 * the same question and both used to answer a bad address with a bare
 * "Invalid URL format.". They were fixed separately, to the same sentence,
 * which is exactly the shape that drifts — so the sentence lives here now.
 */
describe('describeServerUrlError', () => {
  it('names what was typed and what a working address looks like', () => {
    const message = describeServerUrlError('ftp://nope', new Error('Only http and https URLs are supported.'));
    expect(message).toContain('ftp://nope');
    expect(message).toContain('Only http and https URLs are supported.');
    expect(message).toContain('annex.example.com');
  });

  it('quotes the address as typed, not trimmed away to nothing', () => {
    expect(describeServerUrlError('  ftp://nope  ', new Error('boom'))).toContain('"ftp://nope"');
  });

  it('replaces the URL constructor’s bare "Invalid URL"', () => {
    // `new URL('http://')` throws `TypeError: Invalid URL` in Node and
    // "Failed to construct 'URL': Invalid URL" in Chrome. Either way it
    // restates the failure instead of explaining it, and pasting it in leaves
    // the user reading "Invalid URL." inside a sentence that already said the
    // address could not be used.
    const nodeStyle = describeServerUrlError('http://', new TypeError('Invalid URL'));
    expect(nodeStyle).toContain('It could not be parsed.');
    expect(nodeStyle).not.toMatch(/Invalid URL/);

    const chromeStyle = describeServerUrlError('http://', new TypeError("Failed to construct 'URL': Invalid URL"));
    expect(chromeStyle).toContain('It could not be parsed.');
  });

  it('survives a thrown non-Error', () => {
    expect(describeServerUrlError('nope', 'a string')).toContain('It could not be parsed.');
  });

  it('describes what normalizeServerUrl actually throws', () => {
    // Not a hypothetical error object — the real rejection path, so the
    // helper and the parser cannot drift apart.
    try {
      normalizeServerUrl('ftp://files.example.com');
      throw new Error('expected a rejection');
    } catch (err) {
      const message = describeServerUrlError('ftp://files.example.com', err);
      expect(message).toContain('Only http and https URLs are supported.');
      expect(message).not.toContain('It could not be parsed.');
    }
  });
});

describe('normalizeServerUrl — credentials in the address', () => {
  // `https://annex.trusted.example@evil.com` has host `evil.com`; the part a
  // reader takes for the hostname is the username. Everything that shows a
  // server address shows the string — the invite banner, the server hub row,
  // "Could not reach server at …" — and everything that connects uses the
  // host. Refused rather than rewritten: Annex never sends credentials in a
  // URL, so there is nothing here to support.
  it.each([
    'https://annex.trusted.example@evil.com',
    'http://annex.trusted.example@evil.com',
    'https://user:pass@evil.com',
    'annex.trusted.example@evil.com',
  ])('rejects %s', (input) => {
    expect(() => normalizeServerUrl(input)).toThrow(/username or password/i);
  });

  it('still accepts an ordinary address', () => {
    expect(normalizeServerUrl('https://annex.example.com')).toBe('https://annex.example.com');
  });

  it('explains the rejection to the user', () => {
    let message = '';
    try {
      normalizeServerUrl('https://annex.trusted.example@evil.com');
    } catch (err) {
      message = describeServerUrlError('https://annex.trusted.example@evil.com', err);
    }
    expect(message).toMatch(/username or password/i);
  });
});
