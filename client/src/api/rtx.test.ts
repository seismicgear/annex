import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { getPublicAgents, getPublicEvents } from './rtx';

function jsonResponse(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
  } as Response;
}

describe('public observability endpoints', () => {
  beforeEach(() => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it('returns the events array from the normal envelope', async () => {
    global.fetch = vi.fn(async () =>
      jsonResponse({ events: [{ id: 1 }, { id: 2 }], count: 2 }),
    ) as unknown as typeof fetch;

    await expect(getPublicEvents()).resolves.toHaveLength(2);
  });

  // Indexing straight into the envelope produced `undefined`, and the Events
  // tab then died on `events.length` with an uncaught TypeError — a blank
  // screen instead of a degraded one.
  it.each([
    ['a missing field', { count: 0 }],
    ['a null body', null],
    ['a string body', 'not json at all'],
    ['a wrongly-typed field', { events: 'nope' }],
  ])('degrades to an empty list rather than crashing on %s', async (_label, body) => {
    global.fetch = vi.fn(async () => jsonResponse(body)) as unknown as typeof fetch;

    await expect(getPublicEvents()).resolves.toEqual([]);
  });

  it('accepts a bare array, which proxies and fixtures commonly return', async () => {
    global.fetch = vi.fn(async () =>
      jsonResponse([{ id: 1 }]),
    ) as unknown as typeof fetch;

    await expect(getPublicEvents()).resolves.toHaveLength(1);
  });

  it('applies the same tolerance to the agents endpoint', async () => {
    global.fetch = vi.fn(async () => jsonResponse({})) as unknown as typeof fetch;

    await expect(getPublicAgents()).resolves.toEqual({ agents: [] });
  });

  it('passes the domain and limit filters through as query parameters', async () => {
    const fetchMock = vi.fn(async () => jsonResponse({ events: [], count: 0 }));
    global.fetch = fetchMock as unknown as typeof fetch;

    await getPublicEvents('MODERATION', undefined, 25);

    const url = String(fetchMock.mock.calls[0][0]);
    expect(url).toContain('domain=MODERATION');
    expect(url).toContain('limit=25');
  });
});
