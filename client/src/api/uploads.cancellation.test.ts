// @vitest-environment node
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
beforeEach(() => vi.resetModules());
afterEach(() => vi.unstubAllGlobals());
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status, headers: { 'Content-Type': 'application/json' } });
const policy = { images_enabled: true, videos_enabled: true, files_enabled: true, max_image_size_mb: 5, max_video_size_mb: 5, max_file_size_mb: 5 };
it('sends an opaque prepared ID and removes that same upload after a lost committed response', async () => {
  let uploadId = '', bodyWaiting = false;
  const fetchMock = vi.fn(async (url: string, init: RequestInit) => {
    if (url.endsWith('/upload-policy')) return json(policy);
    if (url.endsWith('/upload')) {
      const form = init.body as FormData; uploadId = String(form.get('attachment_id'));
      expect(uploadId).toMatch(/^[0-9a-f-]{36}$/);
      expect(await (form.get('file') as File).text()).toBe('ordinary bytes');
      return { ok: true, json: () => new Promise((_resolve, reject) => {
        bodyWaiting = true; init.signal!.addEventListener('abort', () => reject(new DOMException('Response interrupted', 'AbortError')), { once: true });
      }) } as Response;
    }
    if (init.method === 'DELETE') { expect(init.signal?.aborted).not.toBe(true); return json({ status: 'ok' }); }
    return json({ token: 'grant', expiresInSecs: 300 });
  });
  vi.stubGlobal('fetch', fetchMock);
  const core = await import('./core'); core.setApiBaseUrl('https://annex.example'); core.setSessionToken('token', 'alice');
  const api = await import('./uploads'); const abort = new AbortController();
  const pending = api.uploadChatFile('alice', 'room', new File(['ordinary bytes'], 'ordinary.txt', { type: 'text/plain' }), abort.signal);
  const rejected = expect(pending).rejects.toThrow('Response interrupted');
  await vi.waitFor(() => expect(bodyWaiting).toBe(true)); abort.abort(); await rejected;
  expect(fetchMock.mock.calls.some(([url, init]) => init.method === 'DELETE' && url.endsWith(`/uploads/${uploadId}`))).toBe(true);
  expect((await import('@/lib/upload-cleanup')).getUploadCleanups()).toEqual([]);
});
it('rejects an acknowledgment naming a different upload and cancels only the prepared owned ID', async () => {
  let uploadId = '';
  const fetchMock = vi.fn(async (url: string, init: RequestInit) => {
    if (url.endsWith('/upload-policy')) return json(policy);
    if (url.endsWith('/upload')) { uploadId = String((init.body as FormData).get('attachment_id')); return json({ status: 'ok', upload_id: '11111111-1111-4111-8111-111111111111', url: '/uploads/chat/files/wrong.txt' }); }
    if (init.method === 'DELETE') return json({ status: 'ok' });
    return json({ token: 'grant', expiresInSecs: 300 });
  });
  vi.stubGlobal('fetch', fetchMock);
  const core = await import('./core'); core.setApiBaseUrl('https://annex.example'); core.setSessionToken('token', 'alice');
  await expect((await import('./uploads')).uploadChatImage('alice', 'room', new File(['ordinary bytes'], 'ordinary.txt', { type: 'text/plain' }))).rejects.toThrow('prepared attachment ID');
  const deletions = fetchMock.mock.calls.filter(([, init]) => init.method === 'DELETE');
  expect(deletions).toHaveLength(1); expect(deletions[0][0]).toBe(`https://annex.example/api/channels/room/uploads/${uploadId}`);
});
