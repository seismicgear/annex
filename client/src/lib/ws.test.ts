import { describe, it, expect, vi, beforeEach } from 'vitest';
import { AnnexWebSocket } from './ws';

class MockSocket {
  static OPEN = 1;
  static CLOSED = 3;
  readyState = MockSocket.OPEN;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onclose: ((event: { code: number; reason: string; wasClean: boolean }) => void) | null = null;
  onerror: ((event: unknown) => void) | null = null;
  onmessage: ((event: { data: string }) => void) | null = null;
  close = vi.fn(() => { this.readyState = MockSocket.CLOSED; });
  send = vi.fn((data: string) => { this.sent.push(data); });
}

describe('AnnexWebSocket auth refresh', () => {
  const sockets: MockSocket[] = [];
  beforeEach(() => {
    sockets.length = 0;
    class WebSocketCtor extends MockSocket {
      constructor(_url: string) {
        super();
        sockets.push(this);
      }
    }
    const ctorSpy = vi.fn(WebSocketCtor);
    vi.stubGlobal('WebSocket', ctorSpy as unknown as typeof WebSocket);
    vi.stubGlobal('window', { location: { protocol: 'http:', host: 'localhost:5173' } });
  });

  it('reconnectForAuthRefresh reconnects and resumes tracked subscriptions', () => {
    const ws = new AnnexWebSocket('p1', '', 'old');
    ws.connect();
    sockets[0].onopen?.();
    ws.subscribe('chan-1');
    ws.trackLastMessageId('chan-1', 'm-9');

    ws.reconnectForAuthRefresh('new-token');
    expect(sockets[0].close).toHaveBeenCalled();
    expect(sockets.length).toBe(2);

    sockets[1].onopen?.();
    const secondSent = sockets[1].sent.map((x) => JSON.parse(x));
    expect(secondSent).toContainEqual({ type: 'subscribe', channelId: 'chan-1' });
    expect(secondSent).toContainEqual({ type: 'resume', channelId: 'chan-1', lastMessageId: 'm-9' });
  });
});
