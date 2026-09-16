/**
 * Chat attachments carry a grant; server branding does not.
 *
 * `/uploads` used to be a bare static mount, so a URL was the whole
 * credential: an unauthenticated caller could fetch any private attachment,
 * and a member removed from a channel kept every URL they had ever seen.
 * The server now requires a short-lived signed grant on `/uploads/chat/**`
 * and re-reads membership on each fetch.
 *
 * `resolveUrl` is where the grant is attached, because it is the single place
 * every attachment URL passes through — the alternative was three call sites
 * in `MessageView` and two more elsewhere, which is four opportunities to
 * forget.
 */
import { describe, it, expect, beforeEach, vi, afterEach } from 'vitest';
import {
  resolveUrl,
  setApiBaseUrl,
  setSessionToken,
  clearUploadGrant,
  ensureUploadGrant,
} from './core';

const GRANT = 'a-signed-grant-value';

function mockGrantEndpoint() {
  return vi.fn(async (url: string) => {
    if (String(url).includes('/api/uploads/grant')) {
      return {
        ok: true,
        status: 200,
        json: async () => ({ token: GRANT, expiresInSecs: 900 }),
      } as unknown as Response;
    }
    throw new Error(`unexpected fetch: ${url}`);
  });
}

describe('attachment grants', () => {
  beforeEach(() => {
    clearUploadGrant();
    setApiBaseUrl('');
    vi.stubGlobal('fetch', mockGrantEndpoint());
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    clearUploadGrant();
  });

  it('appends the grant to chat attachment URLs', async () => {
    await ensureUploadGrant();
    const url = resolveUrl('/uploads/chat/images/abc.png');
    expect(url).toBe(`/uploads/chat/images/abc.png?t=${encodeURIComponent(GRANT)}`);
  });

  it('leaves server branding alone', async () => {
    await ensureUploadGrant();
    // Branding is shown on the join screen to people who have no identity yet,
    // so it must remain fetchable without one.
    expect(resolveUrl('/uploads/server/icon.png')).toBe('/uploads/server/icon.png');
  });

  it('leaves ordinary API paths alone', async () => {
    await ensureUploadGrant();
    expect(resolveUrl('/api/channels')).toBe('/api/channels');
  });

  it('returns absolute URLs unchanged', async () => {
    await ensureUploadGrant();
    expect(resolveUrl('https://cdn.example.com/x.png')).toBe('https://cdn.example.com/x.png');
  });

  it('does not break rendering before the grant arrives', () => {
    // `resolveUrl` is called from render and cannot await. A cold cache must
    // produce a usable (if unauthorised) URL rather than throwing and taking
    // the message list down with it.
    const url = resolveUrl('/uploads/chat/images/abc.png');
    expect(url).toBe('/uploads/chat/images/abc.png');
  });

  it('mints only one grant for a burst of attachments', async () => {
    const fetchMock = mockGrantEndpoint();
    vi.stubGlobal('fetch', fetchMock);
    // A channel switch renders many attachments at once.
    await Promise.all([ensureUploadGrant(), ensureUploadGrant(), ensureUploadGrant()]);
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('drops the grant when the session changes', async () => {
    await ensureUploadGrant();
    expect(resolveUrl('/uploads/chat/images/a.png')).toContain('t=');

    // A grant names a pseudonym and an epoch, so it must not outlive its
    // owner. Cleared inside setSessionToken rather than at each call site
    // that clears the token.
    setSessionToken('a-different-session', 'a-different-pseudonym');
    expect(resolveUrl('/uploads/chat/images/a.png')).toBe('/uploads/chat/images/a.png');
  });

  it('survives a failed grant request', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => {
        throw new Error('network down');
      }),
    );
    await expect(ensureUploadGrant()).resolves.toBeUndefined();
    expect(resolveUrl('/uploads/chat/images/a.png')).toBe('/uploads/chat/images/a.png');
  });

  it('respects the API base URL for desktop builds', async () => {
    setApiBaseUrl('https://server.example.com');
    await ensureUploadGrant();
    expect(resolveUrl('/uploads/chat/images/a.png')).toBe(
      `https://server.example.com/uploads/chat/images/a.png?t=${encodeURIComponent(GRANT)}`,
    );
  });
});
