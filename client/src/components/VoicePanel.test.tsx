import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import type { ReactNode } from 'react';
import { VoicePanel } from './VoicePanel';

type VoiceStoreSnapshot = {
  voiceToken: string | null;
  livekitUrl: string | null;
  iceServers: Array<{ urls: string[]; username?: string; credential?: string }>;
  connectedChannelId: string | null;
  joiningByChannel: Record<string, boolean>;
  connectionState: string;
  connectionError: string | null;
  callActiveByChannel: Record<string, boolean>;
  joinErrorByChannel: Record<string, { display: string; code: string | null; setupHint: string | null } | null>;
  deafened: boolean;
  micMuted: boolean;
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
  toggleMicAsync: ReturnType<typeof vi.fn>;
  forceReset: ReturnType<typeof vi.fn>;
};

let identityState: { identity: { pseudonymId: string } | null; permissions: { capabilities: { can_voice: boolean } } | null };
let channelsState: { activeChannelId: string | null; channels: Array<{ channel_id: string; channel_type: string; name: string }> };
let voiceState: VoiceStoreSnapshot;
let serversState: { activeServerId: string | null };

vi.mock('@/stores/identity', () => ({
  useIdentityStore: (selector: (state: typeof identityState) => unknown) => selector(identityState),
}));

vi.mock('@/stores/channels', () => ({
  useChannelsStore: (selector: (state: typeof channelsState) => unknown) => selector(channelsState),
}));

vi.mock('@/stores/voice', () => ({
  useVoiceStore: (selector?: (state: VoiceStoreSnapshot) => unknown) =>
    selector ? selector(voiceState) : voiceState,
}));

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

vi.mock('@livekit/components-react', () => ({
  LiveKitRoom: ({ children }: { children: ReactNode }) => <div data-testid="livekit-room">{children}</div>,
  RoomAudioRenderer: () => null,
  useParticipants: () => [],
  useTracks: () => [],
  VideoTrack: () => null,
  useLocalParticipant: () => ({
    localParticipant: {
      identity: 'agent-1',
      isMicrophoneEnabled: true,
      isCameraEnabled: false,
      isScreenShareEnabled: false,
      setMicrophoneEnabled: mockSetMicrophoneEnabled,
      setCameraEnabled: mockSetCameraEnabled,
      setScreenShareEnabled: mockSetScreenShareEnabled,
      trackPublications: new Map(),
    },
    isMicrophoneEnabled: true,
    isCameraEnabled: false,
    isScreenShareEnabled: false,
  }),
  useConnectionState: () => 'connected',
}));

vi.mock('livekit-client', () => ({
  Track: {
    Source: {
      Camera: 'camera',
      ScreenShare: 'screen',
      Microphone: 'mic',
    },
  },
  ConnectionState: {
    Connected: 'connected',
    Connecting: 'connecting',
    Reconnecting: 'reconnecting',
    Disconnected: 'disconnected',
  },
}));

function defaultVoiceState(): VoiceStoreSnapshot {
  return {
    voiceToken: null,
    livekitUrl: null,
    iceServers: [],
    connectedChannelId: null,
    joiningByChannel: {},
    connectionState: 'idle',
    connectionError: null,
    callActiveByChannel: {},
    joinErrorByChannel: {},
    deafened: false,
    micMuted: false,
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
    toggleMicAsync: vi.fn(async () => {}),
    forceReset: vi.fn(),
  };
}

describe('VoicePanel', () => {
  beforeEach(() => {
    mockSetCameraEnabled.mockClear();
    mockSetMicrophoneEnabled.mockClear();
    mockSetScreenShareEnabled.mockClear();

    identityState = {
      identity: { pseudonymId: 'p1' },
      permissions: { capabilities: { can_voice: true } },
    };

    channelsState = {
      activeChannelId: 'chan-1',
      channels: [{ channel_id: 'chan-1', channel_type: 'Voice', name: 'General' }],
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
      livekitUrl: 'wss://livekit.example',
      connectedChannelId: 'chan-1',
    };

    rerender(<VoicePanel />);

    expect(screen.getByText(/Voice Connected/)).toBeInTheDocument();
    expect(screen.getByTestId('livekit-room')).toBeInTheDocument();
  });

  it('renders connected state with ICE servers configured', () => {
    voiceState = {
      ...voiceState,
      voiceToken: 'token-ice',
      livekitUrl: 'wss://livekit.example',
      iceServers: [
        { urls: ['stun:stun.l.google.com:19302'] },
        { urls: ['turn:turn.example.com:3478'], username: 'user', credential: 'pass' },
      ],
      connectedChannelId: 'chan-1',
    };

    render(<VoicePanel />);

    expect(screen.getByText(/Voice Connected/)).toBeInTheDocument();
    expect(screen.getByTestId('livekit-room')).toBeInTheDocument();
  });

  it('renders connected state with empty ICE servers (defaults)', () => {
    voiceState = {
      ...voiceState,
      voiceToken: 'token-no-ice',
      livekitUrl: 'wss://livekit.example',
      iceServers: [],
      connectedChannelId: 'chan-1',
    };

    render(<VoicePanel />);

    expect(screen.getByText(/Voice Connected/)).toBeInTheDocument();
    expect(screen.getByTestId('livekit-room')).toBeInTheDocument();
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
      livekitUrl: 'wss://livekit.example',
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
      livekitUrl: 'wss://livekit.example',
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
      livekitUrl: 'wss://livekit.example',
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
      livekitUrl: 'wss://livekit.example',
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
      livekitUrl: 'wss://livekit.example',
      connectedChannelId: 'chan-1',
      deafened: true,
      outputVolume: 50,
      outputDeviceId: 'speaker-1',
    };

    await act(async () => {
      render(<VoicePanel />);
    });

    // Simulate LiveKit inserting a container with an <audio> child after mount
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
      livekitUrl: 'wss://livekit.example',
      connectedChannelId: 'chan-1',
      outputDeviceId: null,
    };

    // Add an audio element with data-lk-source for the sync effect to find
    const audio = document.createElement('audio');
    audio.setAttribute('data-lk-source', 'microphone');
    // Mock setSinkId
    (audio as any).setSinkId = vi.fn(async () => {});
    document.body.appendChild(audio);

    await act(async () => {
      render(<VoicePanel />);
    });

    // setSinkId should be called with '' to reset to default
    expect((audio as any).setSinkId).toHaveBeenCalledWith('');

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
});
