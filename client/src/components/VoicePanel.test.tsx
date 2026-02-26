import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import type { ReactNode } from 'react';
import { VoicePanel } from './VoicePanel';

type VoiceStoreSnapshot = {
  voiceToken: string | null;
  livekitUrl: string | null;
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
});
