/**
 * Upload errors reached the user as raw JSON.
 *
 * The upload helpers cannot go through `request()` — a multipart body must not
 * be given a JSON `Content-Type` — so they threw
 * `new ApiError(status, await res.text())`. `MessageInput` renders
 * `err.message` straight into the composer, so an upload rejected by the
 * storage gate showed `Upload failed: {"error":"storage unavailable"}` while
 * every other request in the app decoded the identical body to
 * `storage unavailable`. Rate limiting was missed the same way.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { uploadChatImage, uploadChatFile } from './uploads';
import { ApiError } from './core';

const originalFetch = globalThis.fetch;

function response(status: number, body: string, headers: Record<string, string> = {}) {
  return {
    ok: false,
    status,
    headers: new Headers(headers),
    text: async () => body,
    json: async () => JSON.parse(body),
  } as unknown as Response;
}

beforeEach(() => {
  globalThis.fetch = vi.fn();
});
afterEach(() => {
  globalThis.fetch = originalFetch;
});

const file = () => new File([new Uint8Array([1, 2, 3])], 'a.png', { type: 'image/png' });

describe('upload errors are readable', () => {
  it('decodes a JSON error body instead of showing it', async () => {
    vi.mocked(globalThis.fetch).mockResolvedValue(
      response(507, '{"error":"storage unavailable"}'),
    );
    await expect(uploadChatImage('p1', 'ch1', file())).rejects.toThrow(
      /^storage unavailable$/,
    );
  });

  it('prefers `message` over `error` when both are present', async () => {
    vi.mocked(globalThis.fetch).mockResolvedValue(
      response(400, '{"error":"bad_format","message":"That file type is not allowed."}'),
    );
    await expect(uploadChatFile('p1', 'ch1', file())).rejects.toThrow(
      'That file type is not allowed.',
    );
  });

  it('passes a plain-text body through unchanged', async () => {
    vi.mocked(globalThis.fetch).mockResolvedValue(response(500, 'upstream exploded'));
    await expect(uploadChatImage('p1', 'ch1', file())).rejects.toThrow('upstream exploded');
  });

  it('says when to retry a rate-limited upload, like every other request does', async () => {
    vi.mocked(globalThis.fetch).mockResolvedValue(
      response(429, '{"error":"too many"}', { 'Retry-After': '60' }),
    );
    await expect(uploadChatImage('p1', 'ch1', file())).rejects.toThrow(
      'Rate limit exceeded. Try again in 60 seconds.',
    );
  });

  it('keeps the status on the error so callers can branch on it', async () => {
    vi.mocked(globalThis.fetch).mockResolvedValue(
      response(507, '{"error":"storage unavailable"}'),
    );
    await expect(uploadChatImage('p1', 'ch1', file())).rejects.toSatisfy(
      (e: unknown) => e instanceof ApiError && e.status === 507,
    );
  });
});
