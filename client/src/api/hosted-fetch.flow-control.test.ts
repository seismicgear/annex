// @vitest-environment node
import { afterEach, describe, expect, it, vi } from 'vitest';
import { transportFetch } from './hosted-fetch';
const waitFor = vi.waitFor;

const fixture = vi.hoisted(() => {
  class Tunnel {
    static instances: Tunnel[] = [];
    readyState = 1;
    onopen: (() => void) | null = null;
    onmessage: ((event: { data: string }) => void) | null = null;
    onclose: (() => void) | null = null;
    onerror: (() => void) | null = null;
    frames: { type: string; data?: string }[] = [];
    constructor() { Tunnel.instances.push(this); }
    send(data: string) { this.frames.push(JSON.parse(data)); }
    close() { this.readyState = 3; this.onclose?.(); }
    receive(value: unknown) { this.onmessage?.({ data: JSON.stringify(value) }); }
  }
  return { Tunnel };
});
vi.mock('../lib/host-tunnel', async importOriginal => ({
  ...await importOriginal<typeof import('../lib/host-tunnel')>(), HostTunnel: fixture.Tunnel,
}));
afterEach(() => { fixture.Tunnel.instances = []; vi.unstubAllGlobals(); vi.useRealTimers(); });

describe('hosted transfer flow control', () => {
  const url = 'https://www.monolithannex.com/host/' + 'a'.repeat(64) + '/api/file';
  it('bounds an upload by recipient credit and acknowledges downloaded bytes', async () => {
    const directFetch = vi.fn(); vi.stubGlobal('fetch', directFetch);
    const result = transportFetch(url, { method: 'POST', body: new Uint8Array(5 * 64 * 1024) });
    const socket = fixture.Tunnel.instances[0]; socket.onopen?.();
    socket.receive({ type: 'request_ready', window: 2 });
    const uploaded = () => socket.frames.filter(frame => frame.type === 'http_data');
    await waitFor(() => expect(uploaded()).toHaveLength(2));
    expect(socket.frames.some(frame => frame.type === 'http_end')).toBe(false);
    socket.receive({ type: 'request_ack' });
    await waitFor(() => expect(uploaded()).toHaveLength(3));
    socket.receive({ type: 'request_ack' }); socket.receive({ type: 'request_ack' });
    await waitFor(() => expect(uploaded()).toHaveLength(5));
    await waitFor(() => expect(socket.frames.some(frame => frame.type === 'http_end')).toBe(true));
    socket.receive({ type: 'response', status: 200, headers: { 'content-type': 'text/plain' }, window: 2 });
    for (const text of ['first', 'second', 'third']) socket.receive({ type: 'data', data: btoa(text) });
    expect(socket.frames.filter(frame => frame.type === 'response_ack')).toHaveLength(3);
    socket.receive({ type: 'end' });
    expect(await (await result).text()).toBe('firstsecondthird');
    expect(directFetch).not.toHaveBeenCalled();
  });
  it('cancels a blocked upload without replay or plaintext fallback', async () => {
    const directFetch = vi.fn(); vi.stubGlobal('fetch', directFetch);
    const controller = new AbortController();
    const result = transportFetch(url, { method: 'POST', body: new Uint8Array(3 * 64 * 1024), signal: controller.signal });
    const rejected = expect(result).rejects.toMatchObject({ name: 'AbortError' });
    const socket = fixture.Tunnel.instances[0]; socket.onopen?.(); socket.receive({ type: 'request_ready', window: 1 });
    await waitFor(() => expect(socket.frames.filter(frame => frame.type === 'http_data')).toHaveLength(1));
    controller.abort(); await rejected;
    expect(socket.readyState).toBe(3);
    expect(socket.frames.some(frame => frame.type === 'http_end')).toBe(false);
    expect(directFetch).not.toHaveBeenCalled();
  });
  it.each([
    ['POST', '/api/channels/test/upload', 240_000],
    ['POST', '/api/channels/test/encrypted-upload', 240_000],
    ['GET', '/uploads/chat/files/test.bin', 240_000],
    ['DELETE', '/api/channels/test/uploads/one', 120_000],
    ['POST', '/api/registry/register', 120_000],
  ])('bounds %s %s without replaying an uncertain operation', async (method, path, deadline) => {
    vi.useFakeTimers();
    const result = transportFetch(url.replace('/api/file', path), { method });
    let settled = false; void result.then(() => { settled = true; }, () => { settled = true; });
    const rejected = expect(result).rejects.toThrow('request timed out');
    await vi.advanceTimersByTimeAsync(deadline - 1); expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1); await rejected;
    expect(fixture.Tunnel.instances).toHaveLength(1);
    expect(fixture.Tunnel.instances[0].readyState).toBe(3);
  });
});
