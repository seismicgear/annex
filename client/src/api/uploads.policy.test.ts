// @vitest-environment node
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { checkUploadPolicy } from './uploads';
const policy = { images_enabled: true, videos_enabled: true, files_enabled: true, max_image_size_mb: 1, max_video_size_mb: 2, max_file_size_mb: 5 };
beforeEach(() => vi.resetModules());
afterEach(() => vi.unstubAllGlobals());
it('uses byte signatures and independent category limits, including an exact-boundary file', async () => {
  await expect(checkUploadPolicy(new File([new Uint8Array(5 * 1024 * 1024)], 'exact.bin'), policy)).resolves.toBeUndefined();
  await expect(checkUploadPolicy(new File([new Uint8Array(5 * 1024 * 1024 + 1)], 'over.bin'), policy)).rejects.toThrow('allows 5 MB per file');
  const image = new Uint8Array(1024 * 1024 + 1); image.set([137,80,78,71,13,10,26,10]);
  await expect(checkUploadPolicy(new File([image], 'misleading.bin', { type: 'application/octet-stream' }), policy)).rejects.toThrow('allows 1 MB per image');
  const video = new Uint8Array(2 * 1024 * 1024 + 1); video.set([0x1a,0x45,0xdf,0xa3]);
  await expect(checkUploadPolicy(new File([video], 'video.bin'), policy)).rejects.toThrow('allows 2 MB per video');
});
it('rejects a disabled category even for a tiny file', async () => {
  await expect(checkUploadPolicy(new File(['text'], 'tiny.txt'), { ...policy, files_enabled: false })).rejects.toThrow('File uploads are disabled');
});
it('rejects an oversized ordinary upload without POST, DELETE, or a pending cleanup', async () => {
  const fetch = vi.fn(async (url: string, init?: RequestInit) => {
    if (url.endsWith('/api/uploads/grant')) return new Response(JSON.stringify({ token: 'fixture-grant', expiresInSecs: 300 }));
    expect(url).toBe('https://annex.example/api/channels/room/upload-policy');
    expect(init?.method ?? 'GET').toBe('GET');
    return new Response(JSON.stringify(policy), { headers: { 'Content-Type': 'application/json' } });
  });
  vi.stubGlobal('fetch', fetch);
  const core = await import('./core'); core.setApiBaseUrl('https://annex.example'); core.setSessionToken('test-token', 'alice');
  await expect((await import('./uploads')).uploadChatFile('alice', 'room', new File([new Uint8Array(5 * 1024 * 1024 + 1)], 'oversized.bin'))).rejects.toThrow('allows 5 MB per file');
  expect(fetch.mock.calls.filter(([url]) => url.endsWith('/upload-policy'))).toHaveLength(1);
  expect(fetch.mock.calls.some(([url, init]) => url.endsWith('/upload') || init?.method === 'DELETE')).toBe(false);
  expect((await import('@/lib/upload-cleanup')).getUploadCleanups()).toEqual([]);
});
