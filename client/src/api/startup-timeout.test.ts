import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { request, setApiBaseUrl } from './core';
import { register, requestAuthChallenge, verifyMembership, getCurrentRoot, getMerklePath } from './identity';
import { getServerSummary } from './federation';

describe('startup network deadlines', () => {
  beforeEach(() => { vi.useFakeTimers(); setApiBaseUrl('http://127.0.0.1:39999'); });
  afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

  it.each([
    ['summary', () => getServerSummary(), 15_000],
    ['registration', () => register('a', 1, 1), 30_000],
    ['challenge', () => requestAuthChallenge('a', 'topic'), 30_000],
    ['verification', () => verifyMembership('root', 'a', 'topic', {}, []), 30_000],
    ['root', () => getCurrentRoot(), 30_000],
    ['membership path', () => getMerklePath('a'), 30_000],
  ] as const)('aborts a stalled %s request and returns an actionable failure', async (_name, start, ms) => {
    let signal: AbortSignal;
    vi.stubGlobal('fetch', vi.fn((_url, init: RequestInit) => new Promise((_resolve, reject) => {
      signal = init.signal!;
      signal.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')), { once: true });
    })));
    const assertion = expect(start()).rejects.toThrow('Please retry');
    await vi.advanceTimersByTimeAsync(ms);
    await assertion;
    expect(signal!.aborted).toBe(true);
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('keeps the deadline active while the response body is stalled', async () => {
    vi.stubGlobal('fetch', vi.fn(async (_url, init: RequestInit) => ({
      ok: true,
      json: () => new Promise((_resolve, reject) => {
        init.signal!.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')), { once: true });
      }),
    })));
    const assertion = expect(getServerSummary()).rejects.toThrow('15 seconds');
    await vi.advanceTimersByTimeAsync(15_000);
    await assertion;
  });

  it('preserves caller cancellation and releases the deadline', async () => {
    const cancellation = new AbortController();
    vi.stubGlobal('fetch', vi.fn((_url, init: RequestInit) => new Promise((_resolve, reject) => {
      init.signal!.addEventListener('abort', () => reject(new DOMException('Cancelled by caller', 'AbortError')), { once: true });
    })));
    const assertion = expect(request('/api/test', { signal: cancellation.signal }, 15_000)).rejects.toThrow('Cancelled by caller');
    cancellation.abort();
    await assertion;
    expect(vi.getTimerCount()).toBe(0);
  });
});
