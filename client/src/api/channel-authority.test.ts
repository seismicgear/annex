import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
const own = vi.hoisted(() => ({ ensureCertifiedDevice: vi.fn() }));
vi.mock('@/lib/e2e-store', () => ({ getE2eControls: () => own }));
beforeEach(() => { vi.resetModules(); own.ensureCertifiedDevice.mockReset().mockResolvedValue({ device_id: 'c'.repeat(64), server_identity: 'annex-server:' + 'd'.repeat(64) }); });
afterEach(() => vi.unstubAllGlobals());
const peer = { peer_id: 7, label: 'Partner', base_url: 'https://partner.example', authority_identity: 'annex-server:' + 'a'.repeat(64) };
const channel = { channel_id: 'shared', name: 'Shared', channel_type: 'Text' as const, e2e_enabled: true, initialized: true, head_seq: 3, head_hash: 'b'.repeat(64) };
const authority = { channel_id: channel.channel_id, authority_identity: peer.authority_identity, authority_server: peer.base_url, local_authority: true, initialized: false, head_seq: 0, head_hash: '' };
const accepted = () => ({ operation_id: 'join-request', accepted_seq: 4, accepted_hash: 'e'.repeat(64), event: {
  record_type: 'present', header: { channel_id: 'shared', authority_identity: peer.authority_identity, event_kind: 'state_changed', seq: 4 }, event_hash: 'e'.repeat(64),
  payload: { action: 'member_joined', pseudonym_id: 'alice', requesting_identity: 'annex-server:' + 'd'.repeat(64) },
} });
const json = (value: unknown) => new Response(JSON.stringify(value), { headers: { 'Content-Type': 'application/json' } });
async function setup(fetch: ReturnType<typeof vi.fn>) {
  vi.stubGlobal('fetch', fetch); const core = await import('./core'); core.setApiBaseUrl('https://local.example'); core.setSessionToken('local-token', 'alice');
  return { core, api: await import('./channel-authority') };
}
describe('explicit channel authority API', () => {
  it('refuses a catalog whose selected pinned identity or cursor changed', async () => {
    const { parseAuthorityCatalog } = await import('./channel-authority');
    expect(parseAuthorityCatalog({ peer, channels: [channel], next_after_channel_id: null }, peer).channels).toEqual([channel]);
    expect(() => parseAuthorityCatalog({ peer: { ...peer, authority_identity: 'annex-server:' + 'f'.repeat(64) }, channels: [channel], next_after_channel_id: null }, peer)).toThrow('identity changed');
    expect(() => parseAuthorityCatalog({ peer, channels: [channel], next_after_channel_id: 'shared' }, peer, 'shared')).toThrow('did not advance');
  });
  it('waits for local certified enrollment before using the exact selected instance ID to attach', async () => {
    const fetch = vi.fn(async (url: string, init: RequestInit) => url.endsWith('/attach') ? (expect(init.method).toBe('POST'), expect(own.ensureCertifiedDevice).toHaveBeenCalledOnce(), json(accepted())) : json({ token: 'grant', expiresInSecs: 300 }));
    const { api } = await setup(fetch); await api.attachAuthorityChannel('alice', peer, channel);
    const call = fetch.mock.calls.find(([url]) => url.endsWith('/attach'))!;
    expect(call[0]).toBe('https://local.example/api/federation/channels/shared/attach');
    expect(JSON.parse(String(call[1].body))).toEqual({ peer_id: 7, authority_identity: peer.authority_identity, device_id: 'c'.repeat(64) });
    expect(fetch.mock.calls.some(([url]) => url.endsWith('/key-status'))).toBe(false);
  });
  it('does not treat another member or another accepted sequence as confirmation', async () => {
    const receipt = accepted(); receipt.event.payload.pseudonym_id = 'different';
    const { api } = await setup(vi.fn(async (url: string) => json(url.endsWith('/attach') ? receipt : { token: 'grant', expiresInSecs: 300 })));
    await expect(api.attachAuthorityChannel('alice', peer, channel)).rejects.toThrow('did not confirm');
  });
  it('does not publish a device or submit attach for an uninitialized partner channel', async () => {
    const fetch = vi.fn(async (url: string) => (void url, json({ token: 'grant', expiresInSecs: 300 }))); const { api } = await setup(fetch);
    await expect(api.attachAuthorityChannel('alice', peer, { ...channel, initialized: false })).rejects.toThrow('must initialize');
    expect(own.ensureCertifiedDevice).not.toHaveBeenCalled(); expect(fetch.mock.calls.some(([url]) => url.endsWith('/attach'))).toBe(false);
  });
  it('requires a newly read matching initialized authority after the initialization acknowledgment', async () => {
    const fetch = vi.fn(async (url: string, init: RequestInit) => {
      if (url.endsWith('/initialize')) { expect(JSON.parse(String(init.body))).toEqual({ expected_authority_identity: authority.authority_identity }); return json({ status: 'ok' }); }
      return json(url.endsWith('/authority') ? { ...authority, initialized: true, head_seq: 8, head_hash: 'e'.repeat(64) } : { token: 'grant', expiresInSecs: 300 });
    });
    const { api } = await setup(fetch);
    await expect(api.initializeChannelAuthority('alice', authority)).resolves.toMatchObject({ initialized: true, head_seq: 8 });
    expect(fetch.mock.calls.filter(([url]) => url.includes('/shared/')).map(([url]) => url)).toEqual([
      'https://local.example/api/channels/shared/authority/initialize', 'https://local.example/api/channels/shared/authority',
    ]);
  });
  it('rejects a late catalog after a credential context change', async () => {
    let resolve!: (response: Response) => void;
    const fetch = vi.fn((url: string) => url.includes('/authority-peers/') ? new Promise<Response>(done => { resolve = done; }) : Promise.resolve(json({ token: 'grant', expiresInSecs: 300 })));
    const { api, core } = await setup(fetch); const pending = api.listAuthorityChannels('alice', peer);
    core.setApiBaseUrl('https://another.example'); core.setSessionToken('another-token', 'alice');
    resolve(json({ peer, channels: [channel], next_after_channel_id: null }));
    await expect(pending).rejects.toThrow('superseded');
  });
});
