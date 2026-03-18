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
  })),
}));

describe('channels store', () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it('resetServerState clears all per-server transient state', async () => {
    const { useChannelsStore } = await import('./channels');

    // Simulate some state from server A
    useChannelsStore.setState({
      channels: [{ channel_id: 'ch1', name: 'general', channel_type: 'Text', federation_scope: 'local' } as any],
      activeChannelId: 'ch1',
      messages: [{ message_id: 'msg1', channel_id: 'ch1', sender_pseudonym: 'p1', content: 'hello', reply_to_message_id: null, created_at: '', edited_at: null, deleted_at: null }],
      error: 'some error',
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
    expect(state.loadingOlder).toBe(false);
    expect(state.hasMoreMessages).toBe(true);
  });

  it('loadChannels clears channels on failure', async () => {
    const apiModule = await import('@/lib/api');
    vi.mocked(apiModule.listChannels).mockRejectedValueOnce(new Error('network error'));

    const { useChannelsStore } = await import('./channels');

    // Set some initial channels
    useChannelsStore.setState({
      channels: [{ channel_id: 'old', name: 'old', channel_type: 'Text', federation_scope: 'local' } as any],
    });

    await useChannelsStore.getState().loadChannels('p1');

    expect(useChannelsStore.getState().channels).toEqual([]);
    expect(useChannelsStore.getState().error).toBe('network error');
  });
});
