import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { AnnexWebSocket, type WsStatusHandler } from './ws';
import { beginLiveOperation, isSessionTakenOver, markSessionTakenOver, reclaimSession, TAKEOVER_MESSAGE } from './session-ownership';

const server = 'https://takeover.example';
const identity = 'ownership-test';
class Socket {
  static OPEN = 1;
  static CLOSED = 3;
  static instances: Socket[] = [];
  readyState = 0;
  onopen?: () => void;
  onclose?: (event: { code: number; reason: string; wasClean: boolean }) => void;
  onerror?: (event: unknown) => void;
  onmessage?: (event: { data: string }) => void;
  send = vi.fn();
  close = vi.fn(() => { this.readyState = 3; });
  constructor(public url: string) { Socket.instances.push(this); }
  open() { this.readyState = 1; this.onopen?.(); }
  end(code: number, reason: string) { this.readyState = 3; this.onclose?.({ code, reason, wasClean: true }); }
}

describe('one active identity session', () => {
  beforeEach(() => {
    vi.useFakeTimers();
    Socket.instances = [];
    reclaimSession(server, identity);
    vi.stubGlobal('WebSocket', Socket);
  });
  afterEach(() => {
    reclaimSession(server, identity);
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it('stays quiet after takeover through timers, token refresh and remount until explicit reconnect', () => {
    const first = new AnnexWebSocket(identity, server, 'first-token');
    const status = vi.fn<WsStatusHandler>();
    first.onStatus(status);
    first.connect();
    Socket.instances[0].open();
    first.subscribe('history');
    Socket.instances[0].end(4090, 'identity_active_elsewhere');
    expect(status).toHaveBeenLastCalledWith(false, 'taken_over');
    expect(() => first.send('history', 'blocked')).toThrow(TAKEOVER_MESSAGE);
    first.reconnectForAuthRefresh('refreshed-token');
    first.connect();
    vi.advanceTimersByTime(120_000);
    expect(Socket.instances).toHaveLength(1);
    first.disconnect();
    const remounted = new AnnexWebSocket(identity, server, 'refreshed-token');
    remounted.connect();
    expect(remounted.sessionState).toBe('taken_over');
    expect(Socket.instances).toHaveLength(1);
    remounted.reconnectHere();
    expect(Socket.instances).toHaveLength(2);
    expect(Socket.instances[1].url).toContain('refreshed-token');
    Socket.instances[1].open();
    expect(remounted.connected).toBe(true);
    expect(isSessionTakenOver(server, identity)).toBe(false);
    remounted.disconnect();
  });

  it('reconnects ordinary dropped sockets but treats revoked credentials separately', () => {
    const client = new AnnexWebSocket(identity, server);
    client.connect();
    Socket.instances[0].open();
    Socket.instances[0].end(1006, '');
    vi.advanceTimersByTime(1_000);
    expect(Socket.instances).toHaveLength(2);
    Socket.instances[1].end(4001, 'session_revoked');
    client.reconnectForAuthRefresh('old');
    client.reconnectHere();
    client.connect();
    vi.advanceTimersByTime(120_000);
    expect(client.sessionState).toBe('revoked');
    expect(isSessionTakenOver(server, identity)).toBe(false);
    expect(Socket.instances).toHaveLength(2);
    client.disconnect();
  });

  it('aborts pending writes and refuses new ones only in the displaced identity context', () => {
    const write = beginLiveOperation(server, identity);
    const independent = beginLiveOperation(server, 'other-member');
    markSessionTakenOver(server, identity);
    expect(write.signal.aborted).toBe(true);
    expect(write.signal.reason.message).toBe(TAKEOVER_MESSAGE);
    expect(independent.signal.aborted).toBe(false);
    expect(() => beginLiveOperation(server, identity)).toThrow(TAKEOVER_MESSAGE);
    reclaimSession(server, identity);
    const resumed = beginLiveOperation(server, identity);
    expect(resumed.signal.aborted).toBe(false);
    write.release();
    markSessionTakenOver(server, identity);
    expect(resumed.signal.aborted).toBe(true);
    independent.release(); resumed.release();
  });

  it('persists suspension across a fresh module load in the same browser tab', async () => {
    markSessionTakenOver(server, identity);
    vi.resetModules();
    const fresh = await import('./session-ownership');
    expect(fresh.isSessionTakenOver(server, identity)).toBe(true);
    fresh.reclaimSession(server, identity);
  });
});
