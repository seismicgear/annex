import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@/lib/api', () => ({
  joinVoice: vi.fn(async () => ({ token: 'tok', url: 'wss://lk', ice_servers: [] })),
  leaveVoice: vi.fn(async () => {}),
  getVoiceStatus: vi.fn(async () => ({ participants: 0, active: false })),
}));

describe('voice store', () => {
  beforeEach(() => {
    vi.resetModules();
  });

  it('handleUnexpectedDisconnect clears session state and sets error', async () => {
    const { useVoiceStore } = await import('./voice');

    // Simulate a connected call
    useVoiceStore.setState({
      voiceToken: 'active-token',
      webrtcUrl: 'wss://webrtc.example',
      iceServers: [{ urls: ['stun:stun.l.google.com:19302'] }],
      connectedChannelId: 'chan-1',
      connectionState: 'connected',
      connectionError: null,
    });

    // Trigger unexpected disconnect
    useVoiceStore.getState().handleUnexpectedDisconnect('Connection lost.');

    const state = useVoiceStore.getState();
    expect(state.voiceToken).toBeNull();
    expect(state.webrtcUrl).toBeNull();
    expect(state.iceServers).toEqual([]);
    expect(state.connectedChannelId).toBeNull();
    expect(state.connectionState).toBe('failed');
    expect(state.connectionError).toBe('Connection lost.');
  });

  it('handleUnexpectedDisconnect uses default error message', async () => {
    const { useVoiceStore } = await import('./voice');

    useVoiceStore.setState({
      voiceToken: 'token',
      webrtcUrl: 'wss://lk',
      connectedChannelId: 'chan-1',
      connectionState: 'connected',
    });

    useVoiceStore.getState().handleUnexpectedDisconnect();

    expect(useVoiceStore.getState().connectionError).toBe('Voice disconnected unexpectedly.');
  });

  it('setConnectionState with failed clears session', async () => {
    const { useVoiceStore } = await import('./voice');

    useVoiceStore.setState({
      voiceToken: 'token',
      webrtcUrl: 'wss://lk',
      iceServers: [{ urls: ['turn:turn.example.com'] }],
      connectedChannelId: 'chan-1',
    });

    useVoiceStore.getState().setConnectionState('failed', 'room error');

    const state = useVoiceStore.getState();
    expect(state.voiceToken).toBeNull();
    expect(state.webrtcUrl).toBeNull();
    expect(state.connectedChannelId).toBeNull();
    expect(state.connectionState).toBe('failed');
    expect(state.connectionError).toBe('room error');
  });

  it('forceReset clears everything including error', async () => {
    const { useVoiceStore } = await import('./voice');

    useVoiceStore.setState({
      voiceToken: 'token',
      webrtcUrl: 'wss://lk',
      connectedChannelId: 'chan-1',
      connectionState: 'failed',
      connectionError: 'some error',
      deafened: true,
      micMuted: true,
    });

    useVoiceStore.getState().forceReset();

    const state = useVoiceStore.getState();
    expect(state.voiceToken).toBeNull();
    expect(state.connectionState).toBe('idle');
    expect(state.connectionError).toBeNull();
    expect(state.deafened).toBe(false);
  });

  it('handleUnexpectedDisconnect preserves lastFailedChannelId', async () => {
    const { useVoiceStore } = await import('./voice');

    useVoiceStore.setState({
      voiceToken: 'token',
      webrtcUrl: 'wss://lk',
      connectedChannelId: 'chan-1',
      connectionState: 'connected',
    });

    useVoiceStore.getState().handleUnexpectedDisconnect('Lost connection.');

    const state = useVoiceStore.getState();
    expect(state.connectedChannelId).toBeNull();
    expect(state.lastFailedChannelId).toBe('chan-1');
    expect(state.connectionError).toBe('Lost connection.');
  });

  it('setConnectionState failed preserves lastFailedChannelId', async () => {
    const { useVoiceStore } = await import('./voice');

    useVoiceStore.setState({
      voiceToken: 'token',
      webrtcUrl: 'wss://lk',
      connectedChannelId: 'chan-2',
    });

    useVoiceStore.getState().setConnectionState('failed', 'WebRTC error');

    const state = useVoiceStore.getState();
    expect(state.lastFailedChannelId).toBe('chan-2');
    expect(state.connectedChannelId).toBeNull();
  });

  it('joinCall clears lastFailedChannelId on fresh attempt', async () => {
    const { useVoiceStore } = await import('./voice');

    useVoiceStore.setState({
      lastFailedChannelId: 'chan-old',
      connectionError: 'old error',
    });

    // joinCall will succeed via mock
    await useVoiceStore.getState().joinCall('p1', 'chan-new');

    const state = useVoiceStore.getState();
    expect(state.lastFailedChannelId).toBeNull();
    expect(state.connectionError).toBeNull();
    expect(state.voiceToken).toBe('tok');
  });

  it('leaveCall clears lastFailedChannelId', async () => {
    const { useVoiceStore } = await import('./voice');

    useVoiceStore.setState({
      connectedChannelId: 'chan-1',
      lastFailedChannelId: 'chan-1',
      connectionError: 'dropped',
    });

    await useVoiceStore.getState().leaveCall('p1');

    const state = useVoiceStore.getState();
    expect(state.lastFailedChannelId).toBeNull();
    expect(state.connectionError).toBeNull();
  });

  it('stale joinCall is ignored when a newer request supersedes it', async () => {
    const apiMod = await import('@/lib/api');
    const { useVoiceStore } = await import('./voice');

    // Make joinVoice return slowly for chan-1, instantly for chan-2
    let resolveFirst: ((val: unknown) => void) | null = null;
    vi.mocked(apiMod.joinVoice)
      .mockImplementationOnce(() => new Promise((resolve) => { resolveFirst = resolve; }))
      .mockResolvedValueOnce({ token: 'tok-2', url: 'wss://lk-2', ice_servers: [] });

    // Fire two overlapping joins
    const p1 = useVoiceStore.getState().joinCall('p1', 'chan-1');
    const p2 = useVoiceStore.getState().joinCall('p1', 'chan-2');

    // Second resolves first
    await p2;
    expect(useVoiceStore.getState().connectedChannelId).toBe('chan-2');
    expect(useVoiceStore.getState().voiceToken).toBe('tok-2');

    // Now resolve the first (stale) request
    resolveFirst!({ token: 'tok-1', url: 'wss://lk-1', ice_servers: [] });
    await p1;

    // State should still be from chan-2 (stale ignored)
    expect(useVoiceStore.getState().connectedChannelId).toBe('chan-2');
    expect(useVoiceStore.getState().voiceToken).toBe('tok-2');
  });

  it('toggleMicAsync restores micMuted on failure', async () => {
    const { useVoiceStore } = await import('./voice');

    // Start unmuted
    useVoiceStore.setState({ micMuted: false });

    const fakeLp = {
      isMicrophoneEnabled: true,
      setMicrophoneEnabled: vi.fn(async () => { throw new Error('device lost'); }),
    };

    await expect(useVoiceStore.getState().toggleMicAsync(fakeLp)).rejects.toThrow('device lost');

    // micMuted should be restored to match the real WebRTC state (enabled → not muted)
    expect(useVoiceStore.getState().micMuted).toBe(false);
    expect(useVoiceStore.getState().micToggleError).toBe('device lost');
  });

  it('dismissConnectionError clears lastFailedChannelId and connectionError', async () => {
    const { useVoiceStore } = await import('./voice');

    useVoiceStore.setState({
      lastFailedChannelId: 'chan-1',
      connectionError: 'Dropped',
    });

    useVoiceStore.getState().dismissConnectionError();

    expect(useVoiceStore.getState().lastFailedChannelId).toBeNull();
    expect(useVoiceStore.getState().connectionError).toBeNull();
  });
});
