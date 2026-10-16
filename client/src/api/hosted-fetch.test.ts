import { afterEach, describe, expect, it, vi } from 'vitest';
import { hostedUploadRoute, transportFetch } from './hosted-fetch';
afterEach(() => vi.unstubAllGlobals());
describe('public host upload transport boundary', () => {
  it('keeps the exact server path and query when selecting the upload connection', () => {
    const base = 'https://www.monolithannex.com/host/' + 'a'.repeat(64);
    expect(hostedUploadRoute(base + '/api/channels/one/upload?key=one%2Ftwo', new FormData())).toEqual({
      socket: base.replace('https:', 'wss:') + '/__request', path: '/api/channels/one/upload?key=one%2Ftwo',
    });
  });
  it('preserves fetch for relative, independent, malformed and non-upload requests', async () => {
    const response = new Response('ok'); const fetchMock = vi.fn(async () => response);
    vi.stubGlobal('fetch', fetchMock);
    for (const url of ['/api/upload', 'https://other.example/api/upload', 'https://monolithannex.com/host/short/api/upload', 'http://127.0.0.1:3000/api/upload']) {
      const init = { method: 'POST', body: new FormData() };
      expect(await transportFetch(url, init)).toBe(response);
      expect(fetchMock).toHaveBeenLastCalledWith(url, init);
    }
    expect(hostedUploadRoute('https://monolithannex.com/host/' + 'a'.repeat(64) + '/api/write', '{}')).toBeNull();
  });
});
