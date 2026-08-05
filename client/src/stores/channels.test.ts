import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@/lib/api', () => ({
  listChannels: vi.fn(async () => []),
  getMessages: vi.fn(async () => []),
  joinChannel: vi.fn(async () => {}),
  leaveChannel: vi.fn(async () => {}),
  createChannel: vi.fn(async () => ({ status: 'created' })),
}));

vi.mock('@/lib/ws', () => ({
  AnnexWebSocket: vi.fn().mockImplementation(() => ({
    onStatus: vi.fn(),
    onMessage: vi.fn(),
    connect: vi.fn(),
    disconnect: vi.fn(),
    subscribe: vi.fn(),
    unsubscribe: vi.fn(),
    send: vi.fn(),
    setSessionToken: vi.fn(),
    reconnectForAuthRefresh: vi.fn(),
    connected: false,
  })),
}));

const mockLeaveCall = vi.fn(async () => {});
const mockForceReset = vi.fn();
const mockClearChannelCallState = vi.fn();
vi.mock('@/stores/voice', () => ({
  useVoiceStore: {
    getState: () => ({
      connectedChannelId: 'ch1',
      leaveCall: mockLeaveCall,
      forceReset: mockForceReset,
      clearChannelCallState: mockClearChannelCallState,
    }),
  },
}));

describe('channels store', () => {
  beforeEach(() => {
    vi.resetModules();
    mockLeaveCall.mockClear();
    mockForceReset.mockClear();
    mockClearChannelCallState.mockClear();
  });

  it('resetServerState clears all per-server transient state', async () => {
    const { useChannelsStore } = await import('./channels');

    // Simulate some state from server A
    useChannelsStore.setState({
      channels: [{ channel_id: 'ch1', name: 'general', channel_type: 'Text', federation_scope: 'local' } as unknown as import('./channels').Channel],
      activeChannelId: 'ch1',
      messages: [{ message_id: 'msg1', channel_id: 'ch1', sender_pseudonym: 'p1', content: 'hello', reply_to_message_id: null, created_at: '', edited_at: null, deleted_at: null }],
      error: 'some error',
      composerError: 'send failed',
      loadingOlder: true,
      hasMoreMessages: false,
    });

    // Reset
    useChannelsStore.getState().resetServerState();

    const state = useChannelsStore.getState();
    expect(state.channels).toEqual([]);
    expect(state.activeChannelId).toBeNull();
    expect(state.messages).toEqual([]);
    expect(state.error).toBeNull();
    expect(state.composerError).toBeNull();
    expect(state.loadingOlder).toBe(false);
    expect(state.hasMoreMessages).toBe(true);
  });

  it('loadChannels clears channels on failure', async () => {
    const apiModule = await import('@/lib/api');
    vi.mocked(apiModule.listChannels).mockRejectedValueOnce(new Error('network error'));

    const { useChannelsStore } = await import('./channels');

    // Set some initial channels
    useChannelsStore.setState({
      channels: [{ channel_id: 'old', name: 'old', channel_type: 'Text', federation_scope: 'local' } as Record<string, unknown>],
    });

    await useChannelsStore.getState().loadChannels('p1');

    expect(useChannelsStore.getState().channels).toEqual([]);
    expect(useChannelsStore.getState().error).toBe('network error');
  });

  it('clearComposerError clears only the composer error', async () => {
    const { useChannelsStore } = await import('./channels');

    useChannelsStore.setState({
      error: 'channel list error',
      composerError: 'send failed',
    });

    useChannelsStore.getState().clearComposerError();

    expect(useChannelsStore.getState().composerError).toBeNull();
    expect(useChannelsStore.getState().error).toBe('channel list error');
  });

  it('leaveChannel cleans up voice state when leaving a connected call channel', async () => {
    const apiModule = await import('@/lib/api');
    vi.mocked(apiModule.leaveChannel).mockResolvedValueOnce(undefined);

    const { useChannelsStore } = await import('./channels');

    useChannelsStore.setState({
      activeChannelId: 'ch1',
      messages: [{ message_id: 'msg1', channel_id: 'ch1', sender_pseudonym: 'p1', content: 'hello', reply_to_message_id: null, created_at: '', edited_at: null, deleted_at: null }],
    });

    await useChannelsStore.getState().leaveChannel('p1', 'ch1');

    // Voice cleanup should have been called
    expect(mockLeaveCall).toHaveBeenCalledWith('p1');
    expect(mockForceReset).toHaveBeenCalled();
    expect(mockClearChannelCallState).toHaveBeenCalledWith('ch1');

    // Active channel should be cleared
    expect(useChannelsStore.getState().activeChannelId).toBeNull();
    expect(useChannelsStore.getState().messages).toEqual([]);
  });

  it('sendMessage sets composerError on throw', async () => {
    const { useChannelsStore } = await import('./channels');

    // Simulate being connected with a WS that throws on send
    const mockWs = {
      onStatus: vi.fn(),
      onMessage: vi.fn(),
      connect: vi.fn(),
      disconnect: vi.fn(),
      subscribe: vi.fn(),
      unsubscribe: vi.fn(),
      send: vi.fn(() => { throw new Error('send failed'); }),
      connected: true,
    };
    useChannelsStore.setState({
      ws: mockWs as unknown,
      activeChannelId: 'chan-1',
      wsConnected: true,
    });

    useChannelsStore.getState().sendMessage('hello', 'p1');

    expect(useChannelsStore.getState().composerError).toBe('send failed');
  });

  it('sendMessage returns clientRequestId and adds optimistic message', async () => {
    const { useChannelsStore } = await import('./channels');

    const mockWs = {
      onStatus: vi.fn(),
      onMessage: vi.fn(),
      connect: vi.fn(),
      disconnect: vi.fn(),
      subscribe: vi.fn(),
      unsubscribe: vi.fn(),
      send: vi.fn(() => 'req-123'),
      connected: true,
    };
    useChannelsStore.setState({
      ws: mockWs as unknown,
      activeChannelId: 'chan-1',
      wsConnected: true,
    });

    const reqId = useChannelsStore.getState().sendMessage('hello', 'p1');
    expect(reqId).toBe('req-123');
    expect(useChannelsStore.getState().pendingSends.has('req-123')).toBe(true);
    expect(useChannelsStore.getState().pendingSends.get('req-123')?.content).toBe('hello');
    // Optimistic message should be in the list
    const msgs = useChannelsStore.getState().messages;
    expect(msgs.length).toBe(1);
    expect(msgs[0].content).toBe('hello');
    expect(msgs[0].pending).toBe(true);
    expect(msgs[0].clientRequestId).toBe('req-123');
  });

  it('markChannelRead resets unread count for the channel', async () => {
    const { useChannelsStore } = await import('./channels');

    useChannelsStore.setState({
      unreadCounts: { 'ch1': 5, 'ch2': 3 },
      messages: [{ message_id: 'msg1', channel_id: 'ch1', sender_pseudonym: 'p1', content: 'hi', reply_to_message_id: null, created_at: '' }],
    });

    useChannelsStore.getState().markChannelRead('ch1');

    expect(useChannelsStore.getState().unreadCounts['ch1']).toBe(0);
    expect(useChannelsStore.getState().unreadCounts['ch2']).toBe(3);
  });

  it('markChannelRead is channel-safe when messages include multiple channels', async () => {
    const { useChannelsStore } = await import('./channels');

    useChannelsStore.setState({
      unreadCounts: { A: 2, B: 4 },
      lastReadMessageIds: { B: 'b0' },
      messages: [
        { message_id: 'a1', channel_id: 'A', sender_pseudonym: 'p1', content: 'a', reply_to_message_id: null, created_at: '' },
        { message_id: 'b1', channel_id: 'B', sender_pseudonym: 'p1', content: 'b', reply_to_message_id: null, created_at: '' },
      ],
    });

    useChannelsStore.getState().markChannelRead('A');

    const state = useChannelsStore.getState();
    expect(state.unreadCounts.A).toBe(0);
    expect(state.unreadCounts.B).toBe(4);
    expect(state.lastReadMessageIds.A).toBe('a1');
    expect(state.lastReadMessageIds.B).toBe('b0');
  });

  it('dismissFailedMessage removes a failed optimistic message', async () => {
    const { useChannelsStore } = await import('./channels');

    useChannelsStore.setState({
      messages: [
        { message_id: '', channel_id: 'ch1', sender_pseudonym: 'p1', content: 'oops', reply_to_message_id: null, created_at: '', pending: false, failed: true, clientRequestId: 'req-fail' },
        { message_id: 'msg2', channel_id: 'ch1', sender_pseudonym: 'p2', content: 'ok', reply_to_message_id: null, created_at: '' },
      ],
    });

    useChannelsStore.getState().dismissFailedMessage('req-fail');

    const msgs = useChannelsStore.getState().messages;
    expect(msgs.length).toBe(1);
    expect(msgs[0].message_id).toBe('msg2');
  });

  it('sendTyping debounces and does not throw when no ws', async () => {
    const { useChannelsStore } = await import('./channels');

    // No WS connected — should not throw
    useChannelsStore.setState({ ws: null, activeChannelId: 'ch1' });
    expect(() => useChannelsStore.getState().sendTyping()).not.toThrow();
  });

  it('resolvePendingSend removes and returns the pending entry', async () => {
    const { useChannelsStore } = await import('./channels');

    const pending = { clientRequestId: 'req-1', content: 'test', sentAt: Date.now() };
    useChannelsStore.setState({
      pendingSends: new Map([['req-1', pending]]),
    });

    const resolved = useChannelsStore.getState().resolvePendingSend('req-1');
    expect(resolved).toEqual(pending);
    expect(useChannelsStore.getState().pendingSends.has('req-1')).toBe(false);
  });

  it('resetServerState clears pendingSends', async () => {
    const { useChannelsStore } = await import('./channels');

    useChannelsStore.setState({
      pendingSends: new Map([['req-1', { clientRequestId: 'req-1', content: 'test', sentAt: Date.now() }]]),
    });

    useChannelsStore.getState().resetServerState();
    expect(useChannelsStore.getState().pendingSends.size).toBe(0);
  });

  it('selectChannel ignores stale history response and stale subscribe on rapid switch', async () => {
    const apiModule = await import('@/lib/api');
    const deferred = <T,>() => {
      let resolve!: (value: T) => void;
      const promise = new Promise<T>((res) => { resolve = res; });
      return { promise, resolve };
    };
    const a = deferred<Array<{ message_id: string; channel_id: string; sender_pseudonym: string; content: string; created_at: string }>>();
    const b = deferred<Array<{ message_id: string; channel_id: string; sender_pseudonym: string; content: string; created_at: string }>>();

    vi.mocked(apiModule.getMessages).mockImplementation((_p, channelId) => (
      channelId === 'A' ? a.promise : channelId === 'B' ? b.promise : Promise.resolve([])
    ));

    const { useChannelsStore } = await import('./channels');
    const ws = {
      onStatus: vi.fn(),
      onMessage: vi.fn(),
      connect: vi.fn(),
      disconnect: vi.fn(),
      subscribe: vi.fn(),
      unsubscribe: vi.fn(),
      send: vi.fn(),
    setSessionToken: vi.fn(),
    reconnectForAuthRefresh: vi.fn(),
    connected: false,
      trackLastMessageId: vi.fn(),
    };
    useChannelsStore.setState({ ws: ws as unknown });

    const selectA = useChannelsStore.getState().selectChannel('p1', 'A');
    const selectB = useChannelsStore.getState().selectChannel('p1', 'B');

    await Promise.resolve();
    b.resolve([{ message_id: 'b1', channel_id: 'B', sender_pseudonym: 'p2', content: 'new', created_at: '' }]);
    await selectB;

    a.resolve([{ message_id: 'a1', channel_id: 'A', sender_pseudonym: 'p2', content: 'old', created_at: '' }]);
    await selectA;

    const state = useChannelsStore.getState();
    expect(state.activeChannelId).toBe('B');
    expect(state.messages.map((m) => m.channel_id)).toEqual(['B']);
    expect(ws.subscribe).toHaveBeenCalledTimes(1);
    expect(ws.subscribe).toHaveBeenCalledWith('B');
    expect(ws.trackLastMessageId).toHaveBeenCalledTimes(1);
    expect(ws.trackLastMessageId).toHaveBeenCalledWith('B', 'b1');
  });

  it('selectChannel marks read after successful history load', async () => {
    const apiModule = await import('@/lib/api');
    vi.mocked(apiModule.getMessages).mockResolvedValueOnce([
      { message_id: 'm3', channel_id: 'A', sender_pseudonym: 'p2', content: 'newest', created_at: '' },
      { message_id: 'm2', channel_id: 'A', sender_pseudonym: 'p2', content: 'older', created_at: '' },
    ] as Array<{ message_id: string; channel_id: string; sender_pseudonym: string; content: string; created_at: string }>);

    const { useChannelsStore } = await import('./channels');
    useChannelsStore.setState({
      unreadCounts: { A: 7 },
      lastReadMessageIds: { A: 'm1' },
    });

    await useChannelsStore.getState().selectChannel('p1', 'A');

    const state = useChannelsStore.getState();
    expect(state.unreadCounts.A).toBe(0);
    expect(state.lastReadMessageIds.A).toBe('m3');
  });

  it('selectChannel does not write read marker when history fetch fails', async () => {
    const apiModule = await import('@/lib/api');
    vi.mocked(apiModule.getMessages).mockRejectedValueOnce(new Error('boom'));

    const { useChannelsStore } = await import('./channels');
    useChannelsStore.setState({
      unreadCounts: { A: 5 },
      lastReadMessageIds: { A: 'm1' },
    });

    await useChannelsStore.getState().selectChannel('p1', 'A');

    const state = useChannelsStore.getState();
    expect(state.unreadCounts.A).toBe(5);
    expect(state.lastReadMessageIds.A).toBe('m1');
    expect(state.historyError).toBe('boom');
  });


  it('leaveChannel unsubscribes even when websocket is disconnected', async () => {
    const apiModule = await import('@/lib/api');
    vi.mocked(apiModule.leaveChannel).mockResolvedValueOnce(undefined);
    const { useChannelsStore } = await import('./channels');
    const ws = {
      unsubscribe: vi.fn(),
      connected: false,
    };
    useChannelsStore.setState({ ws: ws as unknown as import('@/lib/ws').AnnexWebSocket, joinedChannelIds: new Set(['A']) });

    await useChannelsStore.getState().leaveChannel('p1', 'A');

    expect(ws.unsubscribe).toHaveBeenCalledWith('A');
  });

  it('loadOlderMessages drops stale response after channel switch', async () => {
    const apiModule = await import('@/lib/api');
    let resolveOlder!: (value: unknown) => void;
    vi.mocked(apiModule.getMessages).mockImplementationOnce(
      () => new Promise((res) => { resolveOlder = res; }) as ReturnType<typeof apiModule.getMessages>,
    );

    const { useChannelsStore } = await import('./channels');
    useChannelsStore.setState({
      activeChannelId: 'A',
      messages: [{ message_id: 'a2', channel_id: 'A', sender_pseudonym: 'p2', content: 'a', reply_to_message_id: null, created_at: '' }],
      hasMoreMessages: true,
    });

    const loading = useChannelsStore.getState().loadOlderMessages('p1');

    // User switches channels while the request is in flight.
    useChannelsStore.setState({
      activeChannelId: 'B',
      messages: [{ message_id: 'b1', channel_id: 'B', sender_pseudonym: 'p2', content: 'b', reply_to_message_id: null, created_at: '' }],
      hasMoreMessages: true,
    });

    resolveOlder([{ message_id: 'a1', channel_id: 'A', sender_pseudonym: 'p2', content: 'stale', reply_to_message_id: null, created_at: '' }]);
    await loading;

    const state = useChannelsStore.getState();
    expect(state.messages.map((m) => m.channel_id)).toEqual(['B']);
    expect(state.hasMoreMessages).toBe(true);
    expect(state.loadingOlder).toBe(false);
  });

  it('loadOlderMessages failure after channel switch does not clobber new channel pagination', async () => {
    const apiModule = await import('@/lib/api');
    let rejectOlder!: (err: Error) => void;
    vi.mocked(apiModule.getMessages).mockImplementationOnce(
      () => new Promise((_res, rej) => { rejectOlder = rej; }) as ReturnType<typeof apiModule.getMessages>,
    );

    const { useChannelsStore } = await import('./channels');
    useChannelsStore.setState({
      activeChannelId: 'A',
      messages: [{ message_id: 'a2', channel_id: 'A', sender_pseudonym: 'p2', content: 'a', reply_to_message_id: null, created_at: '' }],
      hasMoreMessages: true,
    });

    const loading = useChannelsStore.getState().loadOlderMessages('p1');

    useChannelsStore.setState({
      activeChannelId: 'B',
      messages: [{ message_id: 'b1', channel_id: 'B', sender_pseudonym: 'p2', content: 'b', reply_to_message_id: null, created_at: '' }],
      hasMoreMessages: true,
    });

    rejectOlder(new Error('network'));
    await loading;

    expect(useChannelsStore.getState().hasMoreMessages).toBe(true);
    expect(useChannelsStore.getState().loadingOlder).toBe(false);
  });

  it('connectWs subscribes to joined channels and counts unread for background messages', async () => {
    const { AnnexWebSocket } = await import('@/lib/ws');
    const onStatusHandlers: Array<(connected: boolean) => void> = [];
    const onMessageHandlers: Array<(frame: import('@/types').WsReceiveFrame) => void> = [];
    const subscribe = vi.fn();
    vi.mocked(AnnexWebSocket).mockImplementationOnce(function mockAnnexWebSocket() {
      return {
      onStatus: vi.fn((cb: (connected: boolean) => void) => { onStatusHandlers.push(cb); }),
      onMessage: vi.fn((cb: (frame: import('@/types').WsReceiveFrame) => void) => { onMessageHandlers.push(cb); }),
      connect: vi.fn(),
      disconnect: vi.fn(),
      subscribe,
      unsubscribe: vi.fn(),
      send: vi.fn(),
      setSessionToken: vi.fn(),
      reconnectForAuthRefresh: vi.fn(),
      connected: false,
      } as unknown as import('@/lib/ws').AnnexWebSocket;
    });

    const { useChannelsStore } = await import('./channels');
    useChannelsStore.setState({ joinedChannelIds: new Set(['A', 'B']), activeChannelId: 'A', unreadCounts: {} });
    useChannelsStore.getState().connectWs('p1');
    onStatusHandlers[0]?.(true);

    onMessageHandlers[0]?.({
      type: 'message',
      channelId: 'B',
      messageId: 'm1',
      senderPseudonym: 'p2',
      content: 'background',
      createdAt: new Date().toISOString(),
    });
    expect(useChannelsStore.getState().unreadCounts.B).toBe(1);
  });
});


  it('updateWsSessionToken reconnects immediately when ws is connected', async () => {
    const { useChannelsStore } = await import('./channels');
    const ws = {
      reconnectForAuthRefresh: vi.fn(),
      setSessionToken: vi.fn(),
      connected: true,
    };
    useChannelsStore.setState({ ws: ws as unknown as import('@/lib/ws').AnnexWebSocket, wsAuthRefreshing: false });

    useChannelsStore.getState().updateWsSessionToken('new-token');

    expect(ws.reconnectForAuthRefresh).toHaveBeenCalledWith('new-token');
    expect(useChannelsStore.getState().wsAuthRefreshing).toBe(true);
  });

  it('updateWsSessionToken only updates token when ws is disconnected', async () => {
    const { useChannelsStore } = await import('./channels');
    const ws = {
      reconnectForAuthRefresh: vi.fn(),
      setSessionToken: vi.fn(),
      connected: false,
    };
    useChannelsStore.setState({ ws: ws as unknown as import('@/lib/ws').AnnexWebSocket, wsAuthRefreshing: false });

    useChannelsStore.getState().updateWsSessionToken('new-token');

    expect(ws.setSessionToken).toHaveBeenCalledWith('new-token');
    expect(ws.reconnectForAuthRefresh).not.toHaveBeenCalled();
    expect(useChannelsStore.getState().wsAuthRefreshing).toBe(false);
  });

describe('failures that used to look like success', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
  });

  it('reverts an edit that could not be sent, instead of showing it saved', async () => {
    const { useChannelsStore } = await import('./channels');

    const ws = {
      editMessage: vi.fn(() => { throw new Error('socket is closed'); }),
    };
    useChannelsStore.setState({
      ws: ws as never,
      activeChannelId: 'chan-1',
      messages: [
        { message_id: 'm1', channel_id: 'chan-1', sender_pseudonym: 'me',
          content: 'origianl typo', created_at: '2026-01-01 00:00:00' } as never,
      ],
      editError: null,
    });

    useChannelsStore.getState().editMessage('m1', 'original typo fixed');

    const state = useChannelsStore.getState();
    // The correction never reached the server, so it must not be on screen
    // looking applied — it came back on the next reload and the user had no
    // idea their fix was lost.
    expect(state.messages[0].content).toBe('origianl typo');
    expect(state.editError).toBeTruthy();
  });

  it('keeps the optimistic edit when the send succeeds', async () => {
    const { useChannelsStore } = await import('./channels');

    const ws = { editMessage: vi.fn() };
    useChannelsStore.setState({
      ws: ws as never,
      activeChannelId: 'chan-1',
      messages: [
        { message_id: 'm1', channel_id: 'chan-1', sender_pseudonym: 'me',
          content: 'before', created_at: '2026-01-01 00:00:00' } as never,
      ],
      editError: null,
    });

    useChannelsStore.getState().editMessage('m1', 'after');

    const state = useChannelsStore.getState();
    expect(state.messages[0].content).toBe('after');
    expect(state.editError).toBeNull();
  });

  it('a failed scrollback page does not claim the history has ended', async () => {
    const api = await import('@/lib/api');
    const { useChannelsStore } = await import('./channels');

    vi.mocked(api.getMessages).mockRejectedValueOnce(new Error('network'));
    useChannelsStore.setState({
      activeChannelId: 'chan-1',
      messages: [
        { message_id: 'm1', channel_id: 'chan-1', sender_pseudonym: 'me',
          content: 'hi', created_at: '2026-01-01 00:00:00' } as never,
      ],
      loadingOlder: false,
      hasMoreMessages: true,
      olderError: null,
    });

    await useChannelsStore.getState().loadOlderMessages('me');

    const state = useChannelsStore.getState();
    // `hasMoreMessages: false` here would render as "you have reached the
    // beginning of this channel" — a claim the server never made.
    expect(state.hasMoreMessages).toBe(true);
    expect(state.olderError).toBeTruthy();
  });

  it('does not retry automatically after a failed page, but retry works', async () => {
    const api = await import('@/lib/api');
    const { useChannelsStore } = await import('./channels');

    vi.mocked(api.getMessages).mockRejectedValueOnce(new Error('network'));
    useChannelsStore.setState({
      activeChannelId: 'chan-1',
      messages: [
        { message_id: 'm1', channel_id: 'chan-1', sender_pseudonym: 'me',
          content: 'hi', created_at: '2026-01-01 00:00:00' } as never,
      ],
      loadingOlder: false,
      hasMoreMessages: true,
      olderError: null,
    });

    await useChannelsStore.getState().loadOlderMessages('me');
    expect(vi.mocked(api.getMessages)).toHaveBeenCalledTimes(1);

    // Every subsequent scroll event must NOT re-fire the request — that
    // loop is what the old `hasMoreMessages: false` was preventing.
    await useChannelsStore.getState().loadOlderMessages('me');
    await useChannelsStore.getState().loadOlderMessages('me');
    expect(vi.mocked(api.getMessages)).toHaveBeenCalledTimes(1);

    // An explicit retry does re-fire it.
    vi.mocked(api.getMessages).mockResolvedValueOnce([]);
    await useChannelsStore.getState().retryOlderMessages('me');
    expect(vi.mocked(api.getMessages)).toHaveBeenCalledTimes(2);
    expect(useChannelsStore.getState().olderError).toBeNull();
  });

  it('a genuinely short page still ends the history', async () => {
    const api = await import('@/lib/api');
    const { useChannelsStore } = await import('./channels');

    // Fewer than PAGE_SIZE rows means the start of the channel, which is a
    // real end and must stay distinguishable from the failure above.
    vi.mocked(api.getMessages).mockResolvedValueOnce([]);
    useChannelsStore.setState({
      activeChannelId: 'chan-1',
      messages: [
        { message_id: 'm1', channel_id: 'chan-1', sender_pseudonym: 'me',
          content: 'hi', created_at: '2026-01-01 00:00:00' } as never,
      ],
      loadingOlder: false,
      hasMoreMessages: true,
      olderError: null,
    });

    await useChannelsStore.getState().loadOlderMessages('me');

    const state = useChannelsStore.getState();
    expect(state.hasMoreMessages).toBe(false);
    expect(state.olderError).toBeNull();
  });
});
