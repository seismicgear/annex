import { afterEach, describe, expect, it, vi } from 'vitest';
import { transportFetch } from './hosted-fetch';
import { hostedRoute } from '../lib/host-tunnel';
afterEach(() => vi.unstubAllGlobals());
describe('content-blind public host transport boundary', () => {
  it('puts only the host identity on the public socket URL, for every application path', () => {
    const base = 'https://www.monolithannex.com/host/' + 'a'.repeat(64);
    for (const path of ['/api/channels/one/upload?key=one%2Ftwo', '/api/public/server/summary', '/uploads/private.png', '/ws?token=secret']) {
      expect(hostedRoute(base + path)).toEqual({
        socket: base.replace('https:', 'wss:') + '/__tunnel', host: 'a'.repeat(64), path,
      });
    }
  });
  it('preserves fetch for relative and independent server requests', async () => {
    const response = new Response('ok'); const fetchMock = vi.fn(async () => response);
    vi.stubGlobal('fetch', fetchMock);
    for (const url of ['/api/upload', 'https://other.example/api/upload', 'https://monolithannex.com/host/short/api/upload', 'http://127.0.0.1:3000/api/upload']) {
      const init = { method: 'POST', body: new FormData() };
      expect(await transportFetch(url, init)).toBe(response);
      expect(fetchMock).toHaveBeenLastCalledWith(url, init);
    }
  });
  it('does not treat unrelated origins or credential-bearing URLs as the public router', () => {
    const path = '/host/' + 'a'.repeat(64) + '/api/write';
    for (const origin of ['https://evilmonolithannex.com', 'https://monolithannex.com.evil.example', 'http://monolithannex.com', 'https://user:password@monolithannex.com', 'https://monolithannex.com:444']) {
      expect(hostedRoute(origin + path)).toBeNull();
    }
  });
});
