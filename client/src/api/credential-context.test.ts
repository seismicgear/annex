/**
 * A credential belongs to one server and one identity, and to nothing else.
 *
 * `refreshSessionToken()` used to await the network and then write whatever
 * came back straight into the module-global `_sessionToken`. The generation
 * check that guards the auto-refresh loop runs in `startTokenRefresh`, AFTER
 * that mutation has already happened, so it cannot prevent an obsolete
 * response from overwriting the credentials of a server the user has since
 * switched to.
 *
 * The sequence is ordinary: start a refresh against server A, switch to
 * server B while it is in flight, let A's response land. The active target is
 * B and the next `Authorization` header carries A's freshly refreshed bearer
 * token — so the very next request DISCLOSES A's credential to B. A bearer
 * token is all that server needs to act as that identity on A.
 *
 * The same shape applies to the attachment grant: it is a signed capability
 * minted by one server for one identity, and `ensureUploadGrant()` had the
 * same await-then-assign structure.
 *
 * The fix is not a bigger `if` around the assignment. Credentials are bound
 * to an immutable (server, identity) context; every asynchronous credential
 * write validates that its context is still current BEFORE mutating, and a
 * context change invalidates everything minted for the previous one.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

/** A promise plus the handles to settle it later. */
function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}

const A = 'https://a.example';
const B = 'https://b.example';

describe('credentials are bound to one (server, identity) context', () => {
  beforeEach(() => {
    vi.resetModules();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('does not let an in-flight refresh for server A overwrite server B\'s token', async () => {
    const core = await import('./core');
    const gate = deferred<{ ok: boolean; json: () => Promise<unknown> }>();
    // Routed by URL: `setSessionToken` warms an attachment grant, so a mock
    // that answered every call the same way could not tell the refresh from
    // the grant and would prove nothing either way.
    const fetchMock = vi.fn((url: string) =>
      String(url).includes('/api/session/refresh')
        ? gate.promise
        : Promise.reject(new Error('no grant in this test')),
    );
    vi.stubGlobal('fetch', fetchMock);

    core.setApiBaseUrl(A);
    core.setSessionToken('A-token', 'alice');

    // The refresh leaves for A and does not come back yet.
    const inFlight = core.refreshSessionToken();
    const refreshCalls = fetchMock.mock.calls.filter(
      (c) => String(c[0]).includes('/api/session/refresh'),
    );
    expect(refreshCalls).toHaveLength(1);
    expect(refreshCalls[0][0]).toBe(`${A}/api/session/refresh`);

    // The user switches servers while it is out.
    core.setApiBaseUrl(B);
    core.setSessionToken('B-token', 'bob');

    // A answers, late.
    gate.resolve({ ok: true, json: async () => ({ sessionToken: 'A-refreshed' }) });
    await expect(inFlight).rejects.toThrow(/no longer current|stale/i);

    // The active credential is still B's, and B's alone.
    expect(core.getSessionToken()).toBe('B-token');
    expect(core.authHeaders('bob').Authorization).toBe('Bearer B-token');
  });

  it('does not let an in-flight refresh survive an identity switch on the same server', async () => {
    const core = await import('./core');
    const gate = deferred<{ ok: boolean; json: () => Promise<unknown> }>();
    vi.stubGlobal('fetch', vi.fn().mockReturnValue(gate.promise));

    core.setApiBaseUrl(A);
    core.setSessionToken('alice-token', 'alice');
    const inFlight = core.refreshSessionToken();

    // Same server, different person — an overlapping identity selection.
    core.setSessionToken('carol-token', 'carol');

    gate.resolve({ ok: true, json: async () => ({ sessionToken: 'alice-refreshed' }) });
    await expect(inFlight).rejects.toThrow(/no longer current|stale/i);
    expect(core.getSessionToken()).toBe('carol-token');
  });

  it('drops the credential when the server target changes', async () => {
    const core = await import('./core');
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('no network in this test')));

    core.setApiBaseUrl(A);
    core.setSessionToken('A-token', 'alice');
    core.setZkProofPayload('{"root_hex":"aa"}');
    expect(core.authHeaders('alice').Authorization).toBe('Bearer A-token');

    core.setApiBaseUrl(B);

    // Nothing minted for A may be presented to B — not the bearer token, and
    // not the membership proof, which names A's Merkle root and nullifier.
    expect(core.getSessionToken()).toBeNull();
    expect(core.authHeaders('alice').Authorization).toBeUndefined();
    expect(core.getZkProofPayload()).toBeNull();
  });

  it('leaves the credential alone when the base URL is re-set to the same server', async () => {
    const core = await import('./core');
    vi.stubGlobal('fetch', vi.fn().mockRejectedValue(new Error('no network in this test')));

    core.setApiBaseUrl(A);
    core.setSessionToken('A-token', 'alice');
    // Trailing slashes are normalised away; this is the same server.
    core.setApiBaseUrl(`${A}/`);
    expect(core.getSessionToken()).toBe('A-token');
    expect(core.authHeaders('alice').Authorization).toBe('Bearer A-token');
  });

  it('does not let an in-flight upload grant for A be handed to B', async () => {
    const core = await import('./core');
    const gate = deferred<{ ok: boolean; json: () => Promise<unknown> }>();
    // A's grant fetch hangs on the gate; B's (and any later one) never answers,
    // so the only grant that can possibly be cached is A's. If it is, the leak
    // is real.
    let seen = 0;
    vi.stubGlobal('fetch', vi.fn(() => (seen++ === 0 ? gate.promise : new Promise(() => {}))));

    core.setApiBaseUrl(A);
    core.setSessionToken('A-token', 'alice');

    const inFlight = core.ensureUploadGrant();

    core.setApiBaseUrl(B);
    core.setSessionToken('B-token', 'bob');

    gate.resolve({ ok: true, json: async () => ({ token: 'A-grant', expiresInSecs: 900 }) });
    await inFlight;

    // A grant is a signed capability for A's storage. It must not be appended
    // to a URL pointed at B.
    const url = core.resolveUrl('/uploads/chat/pic.png');
    expect(url).not.toContain('A-grant');
    expect(url).toBe(`${B}/uploads/chat/pic.png`);
  });

  it('logout invalidates every credential and cancels the refresh loop', async () => {
    vi.useFakeTimers();
    try {
      const core = await import('./core');
      const fetchMock = vi.fn().mockResolvedValue({
        ok: true,
        json: async () => ({ sessionToken: 'should-never-be-stored' }),
      });
      vi.stubGlobal('fetch', fetchMock);

      core.setApiBaseUrl(A);
      core.setSessionToken('A-token', 'alice');
      core.startTokenRefresh(3600);

      core.setSessionToken(null, null);
      expect(core.getSessionToken()).toBeNull();

      await vi.advanceTimersByTimeAsync(3600 * 1000);
      const refreshCalls = fetchMock.mock.calls.filter(
        (c) => String(c[0]).includes('/api/session/refresh'),
      );
      expect(refreshCalls).toHaveLength(0);
      expect(core.getSessionToken()).toBeNull();
    } finally {
      vi.useRealTimers();
    }
  });
});
