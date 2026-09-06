/**
 * A failed token refresh left the session to die quietly.
 *
 * `startTokenRefresh` fires at 80% of the 1-hour TTL and, on failure, did
 * nothing but hand the error to `onError` — whose entire body was a
 * `console.error`. `setInterval` then waited a further 48 minutes before
 * trying again. The token expires 12 minutes after the first attempt, so a
 * single transient failure bought a **36-minute window in which every API
 * call 401s** while the UI still presents a signed-in, working app. Nothing
 * on screen said otherwise.
 *
 * The refresh endpoint accepts expired-but-validly-signed tokens, so a retry
 * a minute later would very likely have worked. These tests pin the retry
 * (inside the remaining validity window) and pin that `onError` fires only
 * once the retries are genuinely exhausted, so its caller can treat it as
 * "this session is over" rather than "one request failed".
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const TTL = 3600;

describe('startTokenRefresh — one failure is not the end of the session', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.resetModules();
  });
  afterEach(async () => {
    const { stopTokenRefresh } = await import('./core');
    stopTokenRefresh();
    vi.useRealTimers();
  });

  it('retries within the token\'s remaining validity instead of waiting a full cycle', async () => {
    const core = await import('./core');
    core.setSessionToken('tok');

    const fetchMock = vi.fn()
      .mockRejectedValueOnce(new Error('network down'))
      .mockResolvedValueOnce({
        ok: true,
        json: async () => ({ sessionToken: 'fresh' }),
      });
    vi.stubGlobal('fetch', fetchMock);

    const onRefreshed = vi.fn();
    const onError = vi.fn();
    core.startTokenRefresh(TTL, onRefreshed, onError);

    // First scheduled attempt, at 80% of TTL.
    await vi.advanceTimersByTimeAsync(TTL * 0.8 * 1000);
    expect(fetchMock).toHaveBeenCalledTimes(1);
    expect(onError).not.toHaveBeenCalled();

    // The retry must land well inside the 20% of TTL still on the clock —
    // not a full refresh cycle later, by which time the token is dead.
    await vi.advanceTimersByTimeAsync(TTL * 0.2 * 1000);
    expect(fetchMock.mock.calls.length).toBeGreaterThan(1);
    expect(onRefreshed).toHaveBeenCalledWith('fresh');
    expect(onError).not.toHaveBeenCalled();
  });

  it('reports through onError only once the retries are exhausted', async () => {
    const core = await import('./core');
    core.setSessionToken('tok');

    const fetchMock = vi.fn().mockRejectedValue(new Error('network down'));
    vi.stubGlobal('fetch', fetchMock);

    const onError = vi.fn();
    core.startTokenRefresh(TTL, vi.fn(), onError);

    await vi.advanceTimersByTimeAsync(TTL * 0.8 * 1000);
    expect(onError).not.toHaveBeenCalled();

    // Run out the rest of the token's life plus a full further cycle.
    await vi.advanceTimersByTimeAsync(TTL * 1000);
    expect(fetchMock.mock.calls.length).toBeGreaterThan(2);
    expect(onError).toHaveBeenCalledTimes(1);
  });

  it('keeps refreshing on the normal schedule after a recovered failure', async () => {
    const core = await import('./core');
    core.setSessionToken('tok');

    const fetchMock = vi.fn()
      .mockRejectedValueOnce(new Error('blip'))
      .mockResolvedValue({ ok: true, json: async () => ({ sessionToken: 'fresh' }) });
    vi.stubGlobal('fetch', fetchMock);

    const onRefreshed = vi.fn();
    core.startTokenRefresh(TTL, onRefreshed, vi.fn());

    await vi.advanceTimersByTimeAsync(TTL * 0.8 * 1000);
    await vi.advanceTimersByTimeAsync(TTL * 0.2 * 1000);
    expect(onRefreshed).toHaveBeenCalledTimes(1);

    // The next scheduled refresh still happens.
    await vi.advanceTimersByTimeAsync(TTL * 0.8 * 1000);
    expect(onRefreshed).toHaveBeenCalledTimes(2);
  });

  it('stopTokenRefresh cancels a pending retry', async () => {
    const core = await import('./core');
    core.setSessionToken('tok');

    const fetchMock = vi.fn().mockRejectedValue(new Error('network down'));
    vi.stubGlobal('fetch', fetchMock);

    const onError = vi.fn();
    core.startTokenRefresh(TTL, vi.fn(), onError);

    await vi.advanceTimersByTimeAsync(TTL * 0.8 * 1000);
    const afterFirst = fetchMock.mock.calls.length;
    core.stopTokenRefresh();

    await vi.advanceTimersByTimeAsync(TTL * 2 * 1000);
    expect(fetchMock.mock.calls.length).toBe(afterFirst);
    expect(onError).not.toHaveBeenCalled();
  });
});
