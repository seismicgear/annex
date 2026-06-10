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
      url: string;
      constructor(url: string) {
        super();
        this.url = url;
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


describe('AnnexWebSocket stale socket handling', () => {
  const sockets: MockSocket[] = [];

  beforeEach(() => {
    sockets.length = 0;
    class WebSocketCtor extends MockSocket {
      url: string;
      constructor(url: string) {
        super();
        this.url = url;
        sockets.push(this);
      }
    }
    const ctorSpy = vi.fn(WebSocketCtor);
    vi.stubGlobal('WebSocket', ctorSpy as unknown as typeof WebSocket);
    vi.stubGlobal('window', { location: { protocol: 'http:', host: 'localhost:5173' } });
  });

  it('stale close after auth refresh does not spawn a duplicate connection', () => {
    vi.useFakeTimers();
    try {
      const ws = new AnnexWebSocket('p1', '', 'old');
      const statusHandler = vi.fn();
      ws.onStatus(statusHandler);
      ws.connect();
      sockets[0].onopen?.();

      ws.reconnectForAuthRefresh('new-token');
      expect(sockets.length).toBe(2);
      sockets[1].onopen?.();
      statusHandler.mockClear();

      // The old socket's close event arrives asynchronously, after the
      // replacement is already live. It must not flap status or schedule
      // a reconnect (which would open a third, duplicate connection).
      sockets[0].onclose?.({ code: 4001, reason: 'auth_refresh', wasClean: true });
      expect(statusHandler).not.toHaveBeenCalled();

      vi.advanceTimersByTime(60_000);
      expect(sockets.length).toBe(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it('frames from a superseded socket are not dispatched', () => {
    const ws = new AnnexWebSocket('p1', '', 'old');
    const handler = vi.fn();
    ws.onMessage(handler);
    ws.connect();
    sockets[0].onopen?.();

    ws.reconnectForAuthRefresh('new-token');
    sockets[1].onopen?.();

    sockets[0].onmessage?.({ data: JSON.stringify({ type: 'message', channelId: 'c1', content: 'stale' }) });
    expect(handler).not.toHaveBeenCalled();

    sockets[1].onmessage?.({ data: JSON.stringify({ type: 'message', channelId: 'c1', content: 'fresh' }) });
    expect(handler).toHaveBeenCalledTimes(1);
  });

  it('overlapping reconnect schedules collapse into a single pending timer', () => {
    vi.useFakeTimers();
    try {
      const ws = new AnnexWebSocket('p1');
      ws.connect();
      sockets[0].onopen?.();

      // Two close events from the live socket (close + error-close race).
      sockets[0].onclose?.({ code: 1006, reason: '', wasClean: false });
      sockets[0].onclose?.({ code: 1006, reason: '', wasClean: false });

      vi.advanceTimersByTime(60_000);
      // One reconnect attempt, not two: original socket + a single retry.
      expect(sockets.length).toBe(2);
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('AnnexWebSocket frame validation', () => {
  const sockets: MockSocket[] = [];

  beforeEach(() => {
    sockets.length = 0;
    class WebSocketCtor extends MockSocket {
      url: string;
      constructor(url: string) {
        super();
        this.url = url;
        sockets.push(this);
      }
    }
    const ctorSpy = vi.fn(WebSocketCtor);
    vi.stubGlobal('WebSocket', ctorSpy as unknown as typeof WebSocket);
    vi.stubGlobal('window', { location: { protocol: 'http:', host: 'localhost:5173' } });
  });

  it('emits internal warning frame for malformed JSON payloads', () => {
    const ws = new AnnexWebSocket('p1');
    const handler = vi.fn();
    ws.onMessage(handler);

    ws.connect();
    sockets[0].onmessage?.({ data: '{not-json' });

    expect(handler).toHaveBeenCalledWith(expect.objectContaining({
      type: 'internal_error',
      error: 'failed to parse websocket frame as JSON',
      rawPayloadPreview: '{not-json',
    }));
  });

  it('emits internal warning frame for unknown/malformed frame shapes', () => {
    const ws = new AnnexWebSocket('p1');
    const handler = vi.fn();
    ws.onMessage(handler);

    ws.connect();
    sockets[0].onmessage?.({ data: JSON.stringify({ foo: 'bar' }) });

    expect(handler).toHaveBeenCalledWith(expect.objectContaining({
      type: 'internal_error',
      error: 'received malformed websocket frame shape',
    }));
  });
});
