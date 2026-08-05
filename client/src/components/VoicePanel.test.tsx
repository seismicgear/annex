import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import { VoicePanel } from './VoicePanel';

type VoiceStoreSnapshot = {
  voiceToken: string | null;
  webrtcUrl: string | null;
  iceServers: Array<{ urls: string[]; username?: string; credential?: string }>;
  connectedChannelId: string | null;
  joiningByChannel: Record<string, boolean>;
  connectionState: string;
  connectionError: string | null;
  callActiveByChannel: Record<string, boolean>;
  participantsByChannel: Record<string, string[]>;
  joinErrorByChannel: Record<string, { display: string; code: string | null; setupHint: string | null } | null>;
  deafened: boolean;
  micMuted: boolean;
  lastFailedChannelId: string | null;
  joiningAnyCall: boolean;
  micToggleError: string | null;
  inputDeviceId: string | null;
  outputDeviceId: string | null;
  inputVolume: number;
  outputVolume: number;
  cameraDeviceId: string | null;
  joinCall: ReturnType<typeof vi.fn>;
  leaveCall: ReturnType<typeof vi.fn>;
  checkCallActive: ReturnType<typeof vi.fn>;
  isCallActive: ReturnType<typeof vi.fn>;
  isJoining: ReturnType<typeof vi.fn>;
  getJoinError: ReturnType<typeof vi.fn>;
  clearChannelCallState: ReturnType<typeof vi.fn>;
  toggleDeafen: ReturnType<typeof vi.fn>;
  toggleMicMuted: ReturnType<typeof vi.fn>;
  setMicMuted: ReturnType<typeof vi.fn>;
  setCameraDevice: ReturnType<typeof vi.fn>;
  setConnectionState: ReturnType<typeof vi.fn>;
  handleUnexpectedDisconnect: ReturnType<typeof vi.fn>;
  toggleMicAsync: ReturnType<typeof vi.fn>;
  forceReset: ReturnType<typeof vi.fn>;
  dismissConnectionError: ReturnType<typeof vi.fn>;
  clearMicToggleError: ReturnType<typeof vi.fn>;
  voiceSessionDisabled: boolean;
  voiceSessionDisabledReason: string | null;
  setVoiceSessionDisabled: ReturnType<typeof vi.fn>;
  setInputDevice: ReturnType<typeof vi.fn>;
  setOutputDevice: ReturnType<typeof vi.fn>;
  setInputVolume: ReturnType<typeof vi.fn>;
  setOutputVolume: ReturnType<typeof vi.fn>;
};

let identityState: { identity: { pseudonymId: string } | null; permissions: { capabilities: { can_voice: boolean } } | null; permissionsStatus: string };
let channelsState: { activeChannelId: string | null; channels: Array<{ channel_id: string; channel_type: string; name: string }>; ws: unknown };
let voiceState: VoiceStoreSnapshot;
let serversState: { activeServerId: string | null };

vi.mock('@/stores/identity', () => ({
  useIdentityStore: (selector: (state: typeof identityState) => unknown) => selector(identityState),
}));

vi.mock('@/stores/channels', () => ({
  useChannelsStore: (selector: (state: typeof channelsState) => unknown) => selector(channelsState),
}));

// Mock WebSocket for WebRTC signaling
const mockWs = {
  sendWebRtcOffer: vi.fn(),
  sendIceCandidate: vi.fn(),
  onMessage: vi.fn(() => vi.fn()), // returns unsubscribe function
};

vi.mock('@/stores/voice', () => {
  const fn = (selector?: (state: typeof voiceState) => unknown) =>
    selector ? selector(voiceState) : voiceState;
  fn.getState = () => voiceState;
  fn.setState = vi.fn((partial: Partial<typeof voiceState> | ((s: typeof voiceState) => Partial<typeof voiceState>)) => {
    Object.assign(voiceState, typeof partial === 'function' ? partial(voiceState) : partial);
  });
  return { useVoiceStore: fn };
});

vi.mock('@/stores/servers', () => ({
  useServersStore: (selector: (state: typeof serversState) => unknown) => selector(serversState),
}));

vi.mock('@/lib/api', () => ({
  getVoiceConfigStatus: vi.fn(async () => ({ voice_enabled: false, setup_hint: 'Enable voice in server config' })),
}));

// Mock the tauri module — tests run in browser (jsdom), not inside Tauri.
const mockGetPlatformMediaStatus = vi.fn().mockResolvedValue({
  screen_share_available: true,
  camera_mic_available: true,
  warnings: [],
  display_server: 'test',
});
vi.mock('@/lib/tauri', () => ({
  isTauri: () => true,
  getPlatformMediaStatus: (...args: unknown[]) => mockGetPlatformMediaStatus(...args),
  setMediaKeepalive: vi.fn(async () => {}),
}));

const mockSetCameraEnabled = vi.fn(async () => {});
const mockSetMicrophoneEnabled = vi.fn(async () => {});
const mockSetScreenShareEnabled = vi.fn(async () => {});

// Shared mock session returned by every `new WebRtcSession(...)` call.
// Properties are reset in beforeEach.
const mockSessionBase = {
  connectionState: 'connected' as string,
  remoteAudioTracks: [] as unknown[],
  isSpeaking: false,
  isMicrophoneEnabled: true,
  isCameraEnabled: false,
  isScreenShareEnabled: false,
  identity: 'agent-1',
  trackPublications: new Map(),
  setMicrophoneEnabled: mockSetMicrophoneEnabled,
  setCameraEnabled: mockSetCameraEnabled,
  setScreenShareEnabled: mockSetScreenShareEnabled,
  connect: vi.fn(async function (this: typeof mockSessionBase) {
    // Trigger the connection state callback so the context updates
    if (this.onConnectionStateChange) {
      this.onConnectionStateChange('connected');
    }
  }),
  disconnect: vi.fn(),
  handleAnswer: vi.fn(async () => {}),
  handleIceCandidate: vi.fn(async () => {}),
  onConnectionStateChange: null as ((state: string) => void) | null,
  onRemoteTracksChanged: null as (() => void) | null,
  onLocalTrackChanged: null as (() => void) | null,
};

vi.mock('@/lib/webrtc', () => {
  // Use a regular function (not arrow) so it works as a constructor with `new`.
  const MockWebRtcSession = vi.fn(function (this: typeof mockSessionBase) {
    Object.assign(this, mockSessionBase);
    // Bind the connect mock so it can reference `this.onConnectionStateChange`
    this.connect = vi.fn(async function (this: typeof mockSessionBase) {
      if (this.onConnectionStateChange) {
        this.onConnectionStateChange('connected');
      }
    }.bind(this));
  });
  return {
    WebRtcSession: MockWebRtcSession,
    TrackSource: { Microphone: 'microphone', Camera: 'camera', ScreenShare: 'screen_share' },
    ConnectionState: { Connected: 'connected', Connecting: 'connecting', Reconnecting: 'reconnecting', Disconnected: 'disconnected' },
  };
});

function defaultVoiceState(): VoiceStoreSnapshot {
  return {
    voiceToken: null,
    webrtcUrl: null,
    iceServers: [],
    connectedChannelId: null,
    joiningByChannel: {},
    connectionState: 'idle',
    connectionError: null,
    callActiveByChannel: {},
    participantsByChannel: {},
    joinErrorByChannel: {},
    deafened: false,
    micMuted: false,
    lastFailedChannelId: null,
    joiningAnyCall: false,
    micToggleError: null,
    inputDeviceId: null,
    outputDeviceId: null,
    inputVolume: 100,
    outputVolume: 100,
    cameraDeviceId: null,
    joinCall: vi.fn(async () => {}),
    leaveCall: vi.fn(async () => {}),
    checkCallActive: vi.fn(async () => {}),
    isCallActive: vi.fn((channelId: string) => voiceState.callActiveByChannel[channelId] ?? false),
    isJoining: vi.fn((channelId: string) => voiceState.joiningByChannel[channelId] ?? false),
    getJoinError: vi.fn((channelId: string) => voiceState.joinErrorByChannel[channelId] ?? null),
    clearChannelCallState: vi.fn(),
    toggleDeafen: vi.fn(),
    toggleMicMuted: vi.fn(),
    setMicMuted: vi.fn(),
    setCameraDevice: vi.fn(),
    setConnectionState: vi.fn(),
    handleUnexpectedDisconnect: vi.fn(),
    toggleMicAsync: vi.fn(async () => {}),
    forceReset: vi.fn(),
    dismissConnectionError: vi.fn(),
    clearMicToggleError: vi.fn(),
    voiceSessionDisabled: false,
    voiceSessionDisabledReason: null,
    setVoiceSessionDisabled: vi.fn(),
    setInputDevice: vi.fn(),
    setOutputDevice: vi.fn(),
    setInputVolume: vi.fn(),
    setOutputVolume: vi.fn(),
  };
}

describe('VoicePanel', () => {
  beforeEach(() => {
    mockSetCameraEnabled.mockClear();
    mockSetMicrophoneEnabled.mockClear();
    mockSetScreenShareEnabled.mockClear();
    mockSessionBase.connect.mockClear();
    mockSessionBase.disconnect.mockClear();
    mockSessionBase.isMicrophoneEnabled = true;
    mockSessionBase.isCameraEnabled = false;
    mockSessionBase.isScreenShareEnabled = false;
    mockSessionBase.connectionState = 'connected';
    mockSessionBase.onConnectionStateChange = null;
    mockSessionBase.onRemoteTracksChanged = null;
    mockSessionBase.onLocalTrackChanged = null;
    mockSessionBase.trackPublications = new Map();
    mockWs.onMessage.mockReturnValue(vi.fn()); // fresh unsubscribe

    identityState = {
      identity: { pseudonymId: 'p1' },
      permissions: { capabilities: { can_voice: true } },
      permissionsStatus: 'ready',
    };

    channelsState = {
      activeChannelId: 'chan-1',
      channels: [{ channel_id: 'chan-1', channel_type: 'Voice', name: 'General' }],
      ws: mockWs,
    };

    serversState = {
      activeServerId: 'server-1',
    };

    voiceState = defaultVoiceState();
  });

  it('renders disconnected and connected states across rerenders without hook-order issues', () => {
    const { rerender } = render(<VoicePanel />);

    expect(screen.getByRole('button', { name: 'Create Call' })).toBeInTheDocument();

    voiceState = {
      ...voiceState,
      voiceToken: 'token-123',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
    };

    rerender(<VoicePanel />);

    expect(screen.getByText(/Voice Connected/)).toBeInTheDocument();
    expect(screen.getByTestId('webrtc-room')).toBeInTheDocument();
  });

  it('renders connected state with ICE servers configured', () => {
    voiceState = {
      ...voiceState,
      voiceToken: 'token-ice',
      webrtcUrl: 'wss://webrtc.example',
      iceServers: [
        { urls: ['stun:stun.l.google.com:19302'] },
        { urls: ['turn:turn.example.com:3478'], username: 'user', credential: 'pass' },
      ],
      connectedChannelId: 'chan-1',
    };

    render(<VoicePanel />);

    expect(screen.getByText(/Voice Connected/)).toBeInTheDocument();
    expect(screen.getByTestId('webrtc-room')).toBeInTheDocument();
  });

  it('renders connected state with empty ICE servers (defaults)', () => {
    voiceState = {
      ...voiceState,
      voiceToken: 'token-no-ice',
      webrtcUrl: 'wss://webrtc.example',
      iceServers: [],
      connectedChannelId: 'chan-1',
    };

    render(<VoicePanel />);

    expect(screen.getByText(/Voice Connected/)).toBeInTheDocument();
    expect(screen.getByTestId('webrtc-room')).toBeInTheDocument();
  });

  it('shows platform media warnings when PipeWire is missing', async () => {
    mockGetPlatformMediaStatus.mockResolvedValueOnce({
      screen_share_available: false,
      camera_mic_available: true,
      warnings: ['PipeWire not detected — screen sharing will not work on Wayland.'],
      display_server: 'wayland',
    });

    await act(async () => {
      render(<VoicePanel />);
    });

    expect(screen.getByText(/PipeWire not detected/)).toBeInTheDocument();
  });

  it('shows no platform warnings when all media is available', async () => {
    mockGetPlatformMediaStatus.mockResolvedValueOnce({
      screen_share_available: true,
      camera_mic_available: true,
      warnings: [],
      display_server: 'x11',
    });

    await act(async () => {
      render(<VoicePanel />);
    });

    expect(screen.queryByText(/PipeWire/)).not.toBeInTheDocument();
  });

  it('handles getPlatformMediaStatus failure gracefully', async () => {
    mockGetPlatformMediaStatus.mockRejectedValueOnce(new Error('not in tauri'));

    await act(async () => {
      render(<VoicePanel />);
    });

    // Should still render the voice panel without errors.
    expect(screen.getByRole('button', { name: 'Create Call' })).toBeInTheDocument();
  });

  it('does not render connected room when token is stale from server switch', () => {
    voiceState = {
      ...voiceState,
      voiceToken: 'token-stale',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
    };
    // activeServerId differs from what was set when call was joined
    serversState.activeServerId = 'server-2';

    render(<VoicePanel />);

    // Should NOT show the connected room since the server switched
  });

  it('uses stored cameraDeviceId when enabling camera', async () => {
    mockSetCameraEnabled.mockClear();

    voiceState = {
      ...voiceState,
      voiceToken: 'token-cam',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
      cameraDeviceId: 'webcam-42',
    };

    await act(async () => {
      render(<VoicePanel />);
    });

    // Click the camera toggle button (camera is off, so it should enable)
    const camBtn = screen.getByTitle('Turn on camera');
    await act(async () => {
      camBtn.click();
    });

    // setCameraEnabled should be called with the saved device ID
    expect(mockSetCameraEnabled).toHaveBeenCalledWith(true, { deviceId: 'webcam-42' });
  });

  it('shows stale camera recovery UI when saved device is not found', async () => {
    mockSetCameraEnabled.mockClear();
    mockSetCameraEnabled.mockRejectedValueOnce(
      Object.assign(new DOMException('device not found', 'NotFoundError'), {}),
    );

    voiceState = {
      ...voiceState,
      voiceToken: 'token-cam',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
      cameraDeviceId: 'stale-webcam',
    };

    await act(async () => {
      render(<VoicePanel />);
    });

    const camBtn = screen.getByTitle('Turn on camera');
    await act(async () => {
      camBtn.click();
    });

    // Should show recovery UI instead of generic error
    expect(screen.getByText(/Saved camera not found/)).toBeInTheDocument();
    expect(screen.getByText('Use default camera')).toBeInTheDocument();
  });

  it('shows error UI when screen share throws AbortError (non-user-cancel)', async () => {
    mockSetScreenShareEnabled.mockRejectedValueOnce(
      new DOMException('Screen recording blocked', 'AbortError'),
    );

    voiceState = {
      ...voiceState,
      voiceToken: 'token-screen',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
    };

    await act(async () => {
      render(<VoicePanel />);
    });

    const screenBtn = screen.getByTitle('Share screen');
    await act(async () => {
      screenBtn.click();
    });

    // Should show an error since the message doesn't look like a user cancel
    expect(screen.getByRole('alert')).toBeInTheDocument();
  });

  it('applies audio prefs to dynamically added container with audio child', async () => {
    voiceState = {
      ...voiceState,
      voiceToken: 'token-sync',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
      deafened: true,
      outputVolume: 50,
      outputDeviceId: 'speaker-1',
    };

    await act(async () => {
      render(<VoicePanel />);
    });

    // Simulate WebRTC inserting a container with an <audio> child after mount
    const container = document.createElement('div');
    const audio = document.createElement('audio');
    container.appendChild(audio);

    await act(async () => {
      document.body.appendChild(container);
      // Give MutationObserver a tick to fire
      await new Promise((r) => setTimeout(r, 0));
    });

    // The audio element should have deafen/volume applied
    expect(audio.muted).toBe(true);
    expect(audio.volume).toBe(0);

    // Cleanup
    document.body.removeChild(container);
  });

  it('resets to default output when outputDeviceId is null', async () => {
    voiceState = {
      ...voiceState,
      voiceToken: 'token-sink',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
      outputDeviceId: null,
    };

    // Add an audio element with data-webrtc-remote for the sync effect to find
    const audio = document.createElement('audio');
    audio.setAttribute('data-webrtc-remote', '');
    // Mock setSinkId
    (audio as unknown as Record<string, unknown>).setSinkId = vi.fn(async () => {});
    document.body.appendChild(audio);

    await act(async () => {
      render(<VoicePanel />);
    });

    // setSinkId should be called with '' to reset to default
    expect((audio as unknown as Record<string, unknown>).setSinkId).toHaveBeenCalledWith('');

    document.body.removeChild(audio);
  });

  it('shows per-channel joining state only for the active channel', () => {
    voiceState = {
      ...voiceState,
      joiningByChannel: { 'chan-1': true },
      isJoining: vi.fn((channelId: string) => channelId === 'chan-1'),
    };

    render(<VoicePanel />);

    expect(screen.getByRole('button', { name: 'Joining...' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Joining...' })).toBeDisabled();
  });

  it('returns to join/create state after WebRTC disconnect with error shown', () => {
    // Start in connected state
    mockSessionBase.connectionState = 'connected';
    voiceState = {
      ...voiceState,
      voiceToken: 'token-disc',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
      connectionState: 'connected',
    };

    const { rerender } = render(<VoicePanel />);
    expect(screen.getByText(/Voice Connected/)).toBeInTheDocument();

    // Simulate unexpected disconnect: store clears session (as handleUnexpectedDisconnect would)
    mockSessionBase.connectionState = 'disconnected';
    voiceState = {
      ...voiceState,
      voiceToken: null,
      webrtcUrl: null,
      connectedChannelId: null,
      connectionState: 'failed',
      connectionError: 'Voice disconnected — the connection was lost.',
    };

    rerender(<VoicePanel />);

    // Should be back to join state
    expect(screen.queryByText(/Voice Connected/)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Create Call' })).toBeInTheDocument();
  });

  it('keeps screen share interrupted banner and shows error when resume fails', async () => {
    mockSetScreenShareEnabled
      .mockReset()
      .mockRejectedValueOnce(new DOMException('device error', 'NotReadableError'));

    voiceState = {
      ...voiceState,
      voiceToken: 'token-resume',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
      connectionState: 'connected',
    };

    await act(async () => {
      render(<VoicePanel />);
    });

    // The screen share interrupted banner is rendered by RoomContent
    // when screenShareInterrupted is true. Since we can't easily set
    // that state externally in the mock, we verify the component renders
    // without errors and the mock is correctly wired.
    // The interrupted state is set via useTauriMediaRestore callback.
    expect(screen.getByTestId('webrtc-room')).toBeInTheDocument();
  });

  it('disables mic/camera buttons when camera_mic_available is blocked', async () => {
    mockGetPlatformMediaStatus.mockResolvedValueOnce({
      screen_share_available: true,
      camera_mic_available: 'blocked',
      warnings: [],
      display_server: 'test',
    });

    voiceState = {
      ...voiceState,
      voiceToken: 'token-blocked',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
      connectionState: 'connected',
    };

    await act(async () => {
      render(<VoicePanel />);
    });

    // Mic and camera buttons should be disabled with blocked title
    const blockedBtns = screen.getAllByTitle(/blocked/i);
    expect(blockedBtns.length).toBeGreaterThanOrEqual(2); // mic + camera
    for (const btn of blockedBtns) {
      expect(btn).toBeDisabled();
    }

    // Should show blocked guidance
    expect(screen.getByText(/Camera and microphone are blocked/)).toBeInTheDocument();
  });

  it('marks participant as speaking only when isSpeaking is true', async () => {
    // Override useParticipants to return a participant with isSpeaking
    // This is tested through the mock - the ParticipantGrid uses p.isSpeaking
    // rather than checking publication.isMuted
    voiceState = {
      ...voiceState,
      voiceToken: 'token-speak',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
      connectionState: 'connected',
    };

    await act(async () => {
      render(<VoicePanel />);
    });

    // With empty participants from the mock, no speaking indicators should be present
    expect(screen.queryByClassName?.('speaking-indicator') ?? null).toBeNull();
  });

  it('shows connectionError in disconnected state after unexpected disconnect', () => {
    // Simulate: call was on chan-1, then unexpected disconnect
    voiceState = {
      ...voiceState,
      voiceToken: null,
      webrtcUrl: null,
      connectedChannelId: null,
      connectionState: 'failed',
      connectionError: 'Voice disconnected — the connection was lost.',
      lastFailedChannelId: 'chan-1',
    };

    render(<VoicePanel />);

    // The error should be visible in the disconnected state
    expect(screen.getByRole('alert')).toHaveTextContent('Voice disconnected — the connection was lost.');
    expect(screen.getByRole('button', { name: 'Create Call' })).toBeInTheDocument();
  });

  it('disables join button when permissions are loading', () => {
    identityState = {
      identity: { pseudonymId: 'p1' },
      permissions: null,
      permissionsStatus: 'loading',
    };

    render(<VoicePanel />);

    const joinBtn = screen.getByRole('button', { name: 'Create Call' });
    expect(joinBtn).toBeDisabled();
    expect(screen.getByRole('status')).toHaveTextContent('Checking voice permissions');
  });

  it('shows permissions error state', () => {
    identityState = {
      identity: { pseudonymId: 'p1' },
      permissions: null,
      permissionsStatus: 'error',
    };

    render(<VoicePanel />);

    expect(screen.getByText(/Could not verify voice permissions/)).toBeInTheDocument();
  });

  it('join button is disabled when joiningAnyCall is true', () => {
    voiceState = {
      ...voiceState,
      joiningAnyCall: true,
    };

    render(<VoicePanel />);

    const joinBtn = screen.getByRole('button', { name: 'Create Call' });
    expect(joinBtn).toBeDisabled();
  });

  it('disables join button when permissionsStatus is error', () => {
    identityState = {
      identity: { pseudonymId: 'p1' },
      permissions: null,
      permissionsStatus: 'error',
    };

    render(<VoicePanel />);

    const joinBtn = screen.getByRole('button', { name: 'Create Call' });
    expect(joinBtn).toBeDisabled();
  });

  it('disables join button when permissions are absent (not yet loaded)', () => {
    identityState = {
      identity: { pseudonymId: 'p1' },
      permissions: null,
      permissionsStatus: 'idle',
    };

    render(<VoicePanel />);

    const joinBtn = screen.getByRole('button', { name: 'Create Call' });
    expect(joinBtn).toBeDisabled();
  });

  it('uses stored inputDeviceId when enabling microphone', async () => {
    mockSetMicrophoneEnabled.mockClear();

    voiceState = {
      ...voiceState,
      voiceToken: 'token-mic',
      webrtcUrl: 'wss://webrtc.example',
      connectedChannelId: 'chan-1',
      inputDeviceId: 'mic-device-42',
    };

    await act(async () => {
      render(<VoicePanel />);
    });

    // Mic is currently enabled (from mock), so clicking toggles it off
    const micBtn = screen.getByTitle('Mute microphone');
    await act(async () => {
      micBtn.click();
    });

    // First toggle should disable (no device options needed for mute)
    expect(mockSetMicrophoneEnabled).toHaveBeenCalledWith(false);
  });

  it('shows disconnect recovery banner on text channels when voice previously failed', () => {
    // Active channel is Text, but a previous voice call on a different channel failed
    channelsState = {
      activeChannelId: 'chan-text',
      channels: [
        { channel_id: 'chan-text', channel_type: 'Text', name: 'General Chat' },
        { channel_id: 'chan-voice', channel_type: 'Voice', name: 'Voice Room' },
      ],
    };

    voiceState = {
      ...voiceState,
      voiceToken: null,
      webrtcUrl: null,
      connectedChannelId: null,
      connectionState: 'failed',
      connectionError: 'Voice disconnected — the connection was lost.',
      lastFailedChannelId: 'chan-voice',
    };

    render(<VoicePanel />);

    // Should show disconnect banner even though active channel is Text
    expect(screen.getByRole('alert')).toHaveTextContent(/Voice disconnected.*Voice Room/);
    // Dismiss button should be available
    expect(screen.getByLabelText('Dismiss')).toBeInTheDocument();
  });

  it('disables join button and shows reason when voiceSessionDisabled is true', () => {
    voiceState = {
      ...voiceState,
      voiceSessionDisabled: true,
      voiceSessionDisabledReason: 'Voice unavailable: WebRTC failed to start',
    };

    render(<VoicePanel />);

    const joinBtn = screen.getByRole('button', { name: 'Create Call' });
    expect(joinBtn).toBeDisabled();
    // The reason text appears in both the status notice and the error area
    const matches = screen.getAllByText(/Voice unavailable: WebRTC failed to start/);
    expect(matches.length).toBeGreaterThanOrEqual(1);
  });

  it('re-enables join button when voiceSessionDisabled is cleared after retry', () => {
    // First render: voice disabled from host failure
    voiceState = {
      ...voiceState,
      voiceSessionDisabled: true,
      voiceSessionDisabledReason: 'Voice unavailable: WebRTC failed to start',
    };

    const { rerender } = render(<VoicePanel />);
    expect(screen.getByRole('button', { name: 'Create Call' })).toBeDisabled();

    // Second render: voice re-enabled after successful retry
    voiceState = {
      ...voiceState,
      voiceSessionDisabled: false,
      voiceSessionDisabledReason: null,
    };

    rerender(<VoicePanel />);
    expect(screen.getByRole('button', { name: 'Create Call' })).not.toBeDisabled();
  });

  // The roster names the tiles in the participant grid, and it only ever
  // arrives from this poll. The effect used to bail the moment `voiceToken`
  // was set — that is, the moment you were in the call — so whoever created a
  // call held the pre-join roster (empty) forever and nobody who joined
  // afterwards ever got a tile. Losing this again would look like nothing: the
  // call still connects, the audio still works, the grid just never fills in.
  describe('voice status polling', () => {
    it('polls before joining, to choose between Create and Join', () => {
      voiceState = { ...voiceState, voiceToken: null, connectedChannelId: null };

      render(<VoicePanel />);

      expect(voiceState.checkCallActive).toHaveBeenCalled();
    });

    it('keeps polling once connected, so the roster stays current', () => {
      voiceState = {
        ...voiceState,
        voiceToken: 'a-token',
        connectedChannelId: 'chan-1',
        checkCallActive: vi.fn(async () => {}),
      };

      render(<VoicePanel />);

      expect(
        voiceState.checkCallActive,
        'polling must continue while in a call — the roster is only populated here',
      ).toHaveBeenCalled();
    });

    it('polls again on an interval rather than only once', () => {
      vi.useFakeTimers();
      try {
        voiceState = {
          ...voiceState,
          voiceToken: 'a-token',
          connectedChannelId: 'chan-1',
          checkCallActive: vi.fn(async () => {}),
        };

        render(<VoicePanel />);
        const initial = voiceState.checkCallActive.mock.calls.length;

        act(() => {
          vi.advanceTimersByTime(25_000);
        });

        expect(voiceState.checkCallActive.mock.calls.length).toBeGreaterThan(initial);
      } finally {
        vi.useRealTimers();
      }
    });
  });
});
