// @vitest-environment node
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
const device = 'a'.repeat(64);
vi.mock('@/lib/e2e-store', () => ({ getE2eChannelManager: () => ({ ensureDevicePublished: async () => device,
  resolveChannelKey: async () => ({ authorityServer: 'annex-server:' + 'a'.repeat(64), channelId: 'room', epoch: 1, keyId: 'b'.repeat(64) }),
}) }));
beforeEach(() => vi.resetModules());
afterEach(() => vi.unstubAllGlobals());
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
async function setup(fetchMock: ReturnType<typeof vi.fn>) {
  vi.stubGlobal('fetch', fetchMock);
  const core = await import('./core');
  core.setApiBaseUrl('https://one.example'); core.setSessionToken('first-token', 'alice');
  return { core, api: await import('./encrypted-uploads') };
}
describe('encrypted attachment grants', () => {
  it('sends the exact device identity, shares concurrent issuance, and keeps devices distinct', async () => {
    const calls: string[] = [];
    const fetchMock = vi.fn(async (_url: string, init: RequestInit) => {
      const body = JSON.parse(String(init.body));
      if (!body.device_id) return json({ token: 'legacy', expiresInSecs: 300 });
      calls.push(body.device_id); return json({ token: `device-${body.device_id}`, expiresInSecs: 300 });
    });
    const { api } = await setup(fetchMock);
    const tokens = await Promise.all([api.encryptedUploadGrant('alice', device), api.encryptedUploadGrant('alice', device)]);
    expect(tokens).toEqual([`device-${device}`, `device-${device}`]);
    await api.encryptedUploadGrant('alice', 'b'.repeat(64));
    expect(calls).toEqual([device, 'b'.repeat(64)]);
  });
  it('never falls back to a cached local v1 grant when a device grant is refused', async () => {
    const fetchMock = vi.fn(async (_url: string, init: RequestInit) => JSON.parse(String(init.body)).device_id
      ? json({ error: 'device revoked' }, 403) : json({ token: 'legacy', expiresInSecs: 300 }));
    const { api } = await setup(fetchMock);
    await expect(api.encryptedUploadGrant('alice', device)).rejects.toThrow('device revoked');
    expect(fetchMock.mock.calls.every(([url]) => !String(url).includes('/uploads/chat/'))).toBe(true);
  });
  it('rejects an in-flight grant after the server or token context changes', async () => {
    let release!: (response: Response) => void;
    const gate = new Promise<Response>(resolve => { release = resolve; });
    const fetchMock = vi.fn(async (_url: string, init: RequestInit) => JSON.parse(String(init.body)).device_id
      ? gate : json({ token: 'legacy', expiresInSecs: 300 }));
    const { api, core } = await setup(fetchMock);
    const pending = api.encryptedUploadGrant('alice', device);
    const rejected = expect(pending).rejects.toThrow(/superseded/);
    core.setApiBaseUrl('https://two.example'); core.setSessionToken('second-token', 'alice');
    release(json({ token: 'first-server-secret', expiresInSecs: 300 }));
    await rejected;
  });
});

describe('interrupted encrypted upload cleanup', () => {
  const policy = { images_enabled: true, videos_enabled: true, files_enabled: true, max_image_size_mb: 5, max_video_size_mb: 10, max_file_size_mb: 5 };
  it('removes the prepared UUID when cancellation interrupts the response after server commit', async () => {
    let uuid = '', abortResponse!: () => void;
    const fetchMock = vi.fn(async (url: string, init: RequestInit) => {
      if (url.endsWith('/upload-policy')) return json(policy);
      if (url.endsWith('/encrypted-upload')) {
        const form = init.body as FormData; uuid = String(form.get('attachment_id'));
        expect((form.get('file') as File).name).toBe(`${uuid}.bin`);
        const bytes = await (form.get('file') as File).text(); expect(bytes).not.toContain('private bytes');
        return { ok: true, json: () => new Promise((_resolve, reject) => {
          abortResponse = () => reject(new DOMException('Response interrupted', 'AbortError'));
          init.signal!.addEventListener('abort', abortResponse, { once: true });
        }) } as Response;
      }
      if (init.method === 'DELETE') { expect(init.signal?.aborted).not.toBe(true); return json({ status: 'ok' }); }
      return json({ token: 'ordinary-grant', expiresInSecs: 300 });
    });
    const { api } = await setup(fetchMock); const abort = new AbortController();
    const pending = api.uploadEncryptedAttachment('alice', 'room', new File(['private bytes'], 'private.txt', { type: 'text/plain' }), abort.signal);
    const rejected = expect(pending).rejects.toThrow(/interrupted/);
    await vi.waitFor(() => expect(abortResponse).toBeTypeOf('function')); abort.abort(); await rejected;
    expect(fetchMock.mock.calls.some(([url, init]) => init.method === 'DELETE' && url.endsWith(`/uploads/${uuid}`))).toBe(true);
    expect((await import('@/lib/upload-cleanup')).getUploadCleanups()).toEqual([]);
  });
  it('retains a failed cleanup for explicit retry instead of claiming cancellation succeeded', async () => {
    let uuid = '', deletionAttempts = 0;
    const fetchMock = vi.fn(async (url: string, init: RequestInit) => {
      if (url.endsWith('/upload-policy')) return json(policy);
      if (url.endsWith('/encrypted-upload')) { uuid = String((init.body as FormData).get('attachment_id')); throw new DOMException('Upload interrupted', 'AbortError'); }
      if (init.method === 'DELETE') return ++deletionAttempts === 1 ? json({ error: 'Disk cleanup pending' }, 409) : json({ status: 'ok' });
      return json({ token: 'ordinary-grant', expiresInSecs: 300 });
    });
    const { api } = await setup(fetchMock);
    await expect(api.uploadEncryptedAttachment('alice', 'room', new File(['private bytes'], 'private.txt', { type: 'text/plain' }))).rejects.toThrow('interrupted');
    const cleanup = await import('@/lib/upload-cleanup');
    expect(cleanup.getUploadCleanups()).toEqual([expect.objectContaining({ uploadId: uuid, channelId: 'room', pending: true, error: 'Disk cleanup pending' })]);
    await cleanup.removeUnpublishedUpload(cleanup.getUploadCleanups()[0]);
    expect(cleanup.getUploadCleanups()).toEqual([]); expect(deletionAttempts).toBe(2);
  });
  it('never sends an old server cleanup request with the new server credentials', async () => {
    const core = await import('./core');
    const fetchMock = vi.fn(async (url: string, init: RequestInit) => {
      if (url.endsWith('/upload-policy')) return json(policy);
      if (url.endsWith('/encrypted-upload')) {
        expect(init.method).toBe('POST'); core.setApiBaseUrl('https://two.example'); core.setSessionToken('new-token', 'alice'); throw new Error('Old upload disconnected');
      }
      return json({ token: 'ordinary-grant', expiresInSecs: 300 });
    });
    const result = await setup(fetchMock);
    await expect(result.api.uploadEncryptedAttachment('alice', 'room', new File(['private bytes'], 'private.txt', { type: 'text/plain' }))).rejects.toThrow('disconnected');
    expect(fetchMock.mock.calls.filter(([, init]) => init.method === 'DELETE')).toEqual([]);
    const entries = (await import('@/lib/upload-cleanup')).getUploadCleanups();
    expect(entries[0]).toMatchObject({ baseUrl: 'https://one.example', pending: true });
  });
});
