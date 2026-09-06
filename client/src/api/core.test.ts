/**
 * `request` and `requestRemote` — the two doors every non-upload call goes
 * through.
 *
 * They held byte-identical copies of the header logic and had already drifted
 * on error handling: `request` routed 429 through `throwApiError`, which
 * reads `Retry-After` and says how long to wait, while `requestRemote` built
 * the `ApiError` itself and never saw the header. Federation discovery is all
 * remote, so a rate-limited peer lookup told the user to wait without saying
 * how long, in different words from everywhere else in the app.
 *
 * That is the same gap `throwApiError` was extracted to close for the upload
 * helpers. This caller had been left behind.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

function response(status: number, body: string, headers: Record<string, string> = {}) {
  return new Response(body, { status, headers });
}

describe('api/core error handling', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.stubGlobal('fetch', vi.fn());
  });
  afterEach(() => vi.unstubAllGlobals());

  it('tells a remote caller how long to wait, the same as a local one', async () => {
    const { request, requestRemote } = await import('./core');

    // A fresh Response per call: a body can only be read once.
    vi.mocked(fetch).mockImplementation(async () =>
      response(429, '{"error":"slow down"}', { 'Retry-After': '30' }),
    );

    const local = await request('/api/thing').catch((e: Error) => e.message);
    const remote = await requestRemote('https://peer.example', '/api/thing').catch(
      (e: Error) => e.message,
    );

    expect(remote).toBe(local);
    expect(remote).toMatch(/30 seconds/);
  });

  it('carries the status and the raw body on a remote failure', async () => {
    const { requestRemote, ApiError } = await import('./core');
    vi.mocked(fetch).mockImplementation(async () => response(503, '{"error":"peer is down"}'));

    const err = await requestRemote('https://peer.example', '/x').catch((e: unknown) => e);

    expect(err).toBeInstanceOf(ApiError);
    expect((err as InstanceType<typeof ApiError>).status).toBe(503);
    expect((err as Error).message).toBe('peer is down');
  });

  it('labels a body as JSON on both doors, and only when there is one', async () => {
    const { request, requestRemote } = await import('./core');
    vi.mocked(fetch).mockImplementation(async () => response(200, '{}'));

    await request('/a', { method: 'POST', body: '{"x":1}' });
    await requestRemote('https://peer.example', '/a', { method: 'POST', body: '{"x":1}' });
    await request('/b');

    const types = vi
      .mocked(fetch)
      .mock.calls.map(([, init]) => (init?.headers as Headers).get('Content-Type'));
    expect(types).toEqual(['application/json', 'application/json', null]);
  });

  it('does not overwrite a content type the caller chose', async () => {
    // The upload helpers avoid these functions for exactly this reason, but a
    // caller that sets its own type here must keep it too.
    const { request } = await import('./core');
    vi.mocked(fetch).mockImplementation(async () => response(200, '{}'));

    await request('/a', {
      method: 'POST',
      body: 'a=1',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded' },
    });

    const [, init] = vi.mocked(fetch).mock.calls[0];
    expect((init?.headers as Headers).get('Content-Type')).toBe(
      'application/x-www-form-urlencoded',
    );
  });
});
