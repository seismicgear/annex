import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import type { ReactNode } from 'react';
import { VoicePanel } from './VoicePanel';

type VoiceStoreSnapshot = {
  voiceToken: string | null;
  livekitUrl: string | null;
  iceServers: Array<{ urls: string[]; username?: string; credential?: string }>;
  connectedChannelId: string | null;
  joining: boolean;
  callActive: boolean;
  lastJoinError: string | null;
  joinCall: ReturnType<typeof vi.fn>;
  leaveCall: ReturnType<typeof vi.fn>;
  checkCallActive: ReturnType<typeof vi.fn>;
};

let identityState: { identity: { pseudonymId: string } | null; permissions: { capabilities: { can_voice: boolean } } | null };
let channelsState: { activeChannelId: string | null; channels: Array<{ channel_id: string; channel_type: string; name: string }> };
let voiceState: VoiceStoreSnapshot;

vi.mock('@/stores/identity', () => ({
  useIdentityStore: (selector: (state: typeof identityState) => unknown) => selector(identityState),
}));

vi.mock('@/stores/channels', () => ({
  useChannelsStore: (selector: (state: typeof channelsState) => unknown) => selector(channelsState),
}));

vi.mock('@/stores/voice', () => ({
  useVoiceStore: () => voiceState,
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
}));

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
      setMicrophoneEnabled: vi.fn(),
      setCameraEnabled: vi.fn(),
      setScreenShareEnabled: vi.fn(),
    },
  }),
}));

vi.mock('livekit-client', () => ({
  Track: {
    Source: {
      Camera: 'camera',
      ScreenShare: 'screen',
      Microphone: 'mic',
    },
  },
}));

describe('VoicePanel', () => {
  beforeEach(() => {
    identityState = {
      identity: { pseudonymId: 'p1' },
      permissions: { capabilities: { can_voice: true } },
    };

    channelsState = {
      activeChannelId: 'chan-1',
      channels: [{ channel_id: 'chan-1', channel_type: 'Voice', name: 'General' }],
    };

    voiceState = {
      voiceToken: null,
      livekitUrl: null,
      iceServers: [],
      connectedChannelId: null,
      joining: false,
      callActive: false,
      lastJoinError: null,
      joinCall: vi.fn(async () => {}),
      leaveCall: vi.fn(async () => {}),
      checkCallActive: vi.fn(async () => {}),
    };
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
});
