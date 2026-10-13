import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
const renewal = vi.hoisted(() => vi.fn());
vi.mock('@/lib/renew-membership', () => ({ renewMembershipAssertion: renewal }));
const json = (status: number, value: unknown) => new Response(JSON.stringify(value), { status });
beforeEach(() => { vi.resetModules(); renewal.mockReset(); vi.stubGlobal('fetch', vi.fn()); });
afterEach(() => vi.unstubAllGlobals());
async function setup() {
  const core = await import('./core');
  // Allow the independently warmed attachment grant to finish before counting.
  vi.mocked(fetch).mockResolvedValue(json(200, { token: 'upload', expiresInSecs: 300 }));
  core.setApiBaseUrl('https://same.example'); core.setSessionToken('session-epoch-1', 'owner'); core.setZkProofPayload('old-proof');
  await vi.waitFor(() => expect(fetch).toHaveBeenCalled());
  await Promise.resolve(); vi.mocked(fetch).mockClear();
  return core;
}
describe('exact retired-root assertion recovery', () => {
  it('single-flights parallel refusals, preserves session and retries each identical write once with the new proof', async () => {
    const core = await setup();
    const calls: { url: string; method?: string; body?: BodyInit | null; token: string | null; proof: string | null }[] = [];
    vi.mocked(fetch).mockImplementation(async (url, init) => {
      const headers = new Headers(init?.headers);
      calls.push({ url: String(url), method: init?.method, body: init?.body, token: headers.get('Authorization'), proof: headers.get('x-annex-zk-proof') });
      return headers.get('x-annex-zk-proof') === btoa('old-proof') ? json(403, { error: 'zk_root_retired' }) : json(200, { ok: true });
    });
    let finish!: () => void;
    renewal.mockImplementation(() => new Promise<void>(resolve => { finish = () => { core.setZkProofPayload('fresh-proof'); resolve(); }; }));
    const body = JSON.stringify({ content: 'exact-ciphertext', attachment_ids: ['same-id'] });
    const first = core.request('/write', { method: 'POST', body, headers: core.authHeaders('owner') });
    const second = core.request('/write', { method: 'POST', body, headers: core.authHeaders('owner') });
    await vi.waitFor(() => expect(renewal).toHaveBeenCalledTimes(1)); finish();
    await expect(Promise.all([first, second])).resolves.toEqual([{ ok: true }, { ok: true }]);
    expect(calls).toHaveLength(4);
    expect(calls.every(call => call.url === 'https://same.example/write' && call.method === 'POST' && call.body === body && call.token === 'Bearer session-epoch-1')).toBe(true);
    expect(calls.filter(call => call.proof === btoa('fresh-proof'))).toHaveLength(2);
    expect(core.getSessionToken()).toBe('session-epoch-1');
  });

  it.each([{ error: 'forbidden' }, { code: 'zk_root_retired' }, { error: 'ZK proof root is not currently acceptable' }])('does not re-prove generic/malformed refusals %#', async body => {
    const core = await setup(); vi.mocked(fetch).mockResolvedValue(json(403, body));
    await expect(core.request('/read', { headers: core.authHeaders('owner') })).rejects.toMatchObject({ status: 403 });
    expect(renewal).not.toHaveBeenCalled(); expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('stops after the second refusal and does not loop through crypto work', async () => {
    const core = await setup(); vi.mocked(fetch).mockImplementation(async () => json(403, { error: 'zk_root_retired' }));
    renewal.mockImplementation(async () => { core.setZkProofPayload('fresh-proof'); });
    await expect(core.request('/read', { headers: core.authHeaders('owner') })).rejects.toMatchObject({ status: 403 });
    expect(renewal).toHaveBeenCalledTimes(1); expect(fetch).toHaveBeenCalledTimes(2);
  });

  it.each(['logout', 'epoch', 'server'] as const)('does not retry after %s changes while proving', async kind => {
    const core = await setup(); vi.mocked(fetch).mockImplementation(async () => json(403, { error: 'zk_root_retired' }));
    renewal.mockImplementation(async () => {
      if (kind === 'logout') core.setSessionToken(null, null);
      else if (kind === 'epoch') core.setSessionToken('session-epoch-2', 'owner');
      else core.setApiBaseUrl('https://other.example');
    });
    await expect(core.request('/read', { headers: core.authHeaders('owner') })).rejects.toThrow('context');
    expect(vi.mocked(fetch).mock.calls.filter(([url]) => String(url).endsWith('/read'))).toHaveLength(1);
  });

  it('does not regenerate for a remote origin or a request carrying someone else’s credentials', async () => {
    const core = await setup(); vi.mocked(fetch).mockImplementation(async () => json(403, { error: 'zk_root_retired' }));
    await core.fetchAuthenticated('https://other.example/read', { headers: core.authHeaders('owner') });
    await core.fetchAuthenticated('https://same.example/read', { headers: { Authorization: 'Bearer other', 'x-annex-zk-proof': btoa('old-proof') } });
    expect(renewal).not.toHaveBeenCalled(); expect(fetch).toHaveBeenCalledTimes(2);
  });
});
