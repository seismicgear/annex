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
      livekitUrl: 'wss://livekit.example',
      iceServers: [{ urls: ['stun:stun.l.google.com:19302'] }],
      connectedChannelId: 'chan-1',
      connectionState: 'connected',
      connectionError: null,
    });

    // Trigger unexpected disconnect
    useVoiceStore.getState().handleUnexpectedDisconnect('Connection lost.');

    const state = useVoiceStore.getState();
    expect(state.voiceToken).toBeNull();
    expect(state.livekitUrl).toBeNull();
    expect(state.iceServers).toEqual([]);
    expect(state.connectedChannelId).toBeNull();
    expect(state.connectionState).toBe('failed');
    expect(state.connectionError).toBe('Connection lost.');
  });

  it('handleUnexpectedDisconnect uses default error message', async () => {
    const { useVoiceStore } = await import('./voice');

    useVoiceStore.setState({
      voiceToken: 'token',
      livekitUrl: 'wss://lk',
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
      livekitUrl: 'wss://lk',
      iceServers: [{ urls: ['turn:turn.example.com'] }],
      connectedChannelId: 'chan-1',
    });

    useVoiceStore.getState().setConnectionState('failed', 'room error');

    const state = useVoiceStore.getState();
    expect(state.voiceToken).toBeNull();
    expect(state.livekitUrl).toBeNull();
    expect(state.connectedChannelId).toBeNull();
    expect(state.connectionState).toBe('failed');
    expect(state.connectionError).toBe('room error');
  });

  it('forceReset clears everything including error', async () => {
    const { useVoiceStore } = await import('./voice');

    useVoiceStore.setState({
      voiceToken: 'token',
      livekitUrl: 'wss://lk',
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
});
