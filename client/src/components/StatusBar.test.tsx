import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { StatusBar } from './StatusBar';

// ── Mock state ──

let identityState: {
  identity: { id: string; pseudonymId: string; serverSlug: string } | null;
  logout: ReturnType<typeof vi.fn>;
  exportCurrent: ReturnType<typeof vi.fn>;
};

let channelsState: {
  wsConnected: boolean;
  channels: Array<{ channel_id: string; name: string }>;
};

let voiceState: {
  voiceToken: string | null;
  connectedChannelId: string | null;
  connectionState: string;
  deafened: boolean;
  micMuted: boolean;
  leaveCall: ReturnType<typeof vi.fn>;
  toggleDeafen: ReturnType<typeof vi.fn>;
  toggleMicMuted: ReturnType<typeof vi.fn>;
  micToggleError: string | null;
  clearMicToggleError: ReturnType<typeof vi.fn>;
};

vi.mock('@/stores/identity', () => ({
  useIdentityStore: (selector: (state: typeof identityState) => unknown) => selector(identityState),
}));

vi.mock('@/stores/channels', () => ({
  useChannelsStore: (selector: (state: typeof channelsState) => unknown) => selector(channelsState),
}));

vi.mock('@/stores/voice', () => ({
  useVoiceStore: () => voiceState,
}));

vi.mock('@/lib/personas', () => ({
  getPersonasForIdentity: vi.fn(async () => []),
}));

vi.mock('@/lib/tauri', () => ({
  isTauri: () => false,
  exportIdentityJson: vi.fn(async () => null),
}));

vi.mock('@/components/DeviceLinkDialog', () => ({
  DeviceLinkDialog: () => null,
}));

vi.mock('@/components/IdentitySettings', () => ({
  IdentitySettings: () => null,
}));

vi.mock('@/components/SocialRecoveryDialog', () => ({
  SocialRecoveryDialog: () => null,
}));

vi.mock('@/components/AudioSettings', () => ({
  AudioSettings: () => null,
}));

describe('StatusBar voice strip', () => {
  beforeEach(() => {
    identityState = {
      identity: { id: 'id-1', pseudonymId: 'p1', serverSlug: 'test' },
      logout: vi.fn(),
      exportCurrent: vi.fn(() => '{}'),
    };

    channelsState = {
      wsConnected: true,
      channels: [{ channel_id: 'chan-1', name: 'General' }],
    };

    voiceState = {
      voiceToken: 'token-123',
      connectedChannelId: 'chan-1',
      connectionState: 'connected',
      deafened: false,
      micMuted: false,
      leaveCall: vi.fn(async () => {}),
      toggleDeafen: vi.fn(),
      toggleMicMuted: vi.fn(),
      micToggleError: null,
      clearMicToggleError: vi.fn(),
    };
  });

  it('renders voice strip when in a call', () => {
    render(<StatusBar />);
    expect(screen.getByText('Voice Connected')).toBeInTheDocument();
    expect(screen.getByText('General')).toBeInTheDocument();
  });

  it('does not render voice strip when not in a call', () => {
    voiceState.voiceToken = null;
    voiceState.connectedChannelId = null;
    render(<StatusBar />);
    expect(screen.queryByText('Voice Connected')).not.toBeInTheDocument();
  });

  it('calls toggleMicMuted from the voice store when mic button is clicked', () => {
    render(<StatusBar />);
    const micButton = screen.getByTitle('Mute microphone');
    fireEvent.click(micButton);
    expect(voiceState.toggleMicMuted).toHaveBeenCalledTimes(1);
  });

  it('calls toggleDeafen from the voice store when deafen button is clicked', () => {
    render(<StatusBar />);
    const deafenButton = screen.getByTitle('Deafen — mute all incoming audio');
    fireEvent.click(deafenButton);
    expect(voiceState.toggleDeafen).toHaveBeenCalledTimes(1);
  });

  it('calls leaveCall from the voice store when disconnect is clicked', () => {
    render(<StatusBar />);
    const disconnectButton = screen.getByTitle('Disconnect from voice channel');
    fireEvent.click(disconnectButton);
    expect(voiceState.leaveCall).toHaveBeenCalledWith('p1');
  });

  it('reflects muted state on mic button', () => {
    voiceState.micMuted = true;
    render(<StatusBar />);
    const micButton = screen.getByTitle('Unmute microphone');
    expect(micButton.className).toContain('muted');
  });

  it('reflects deafened state on deafen button', () => {
    voiceState.deafened = true;
    render(<StatusBar />);
    const deafenButton = screen.getByTitle('Undeafen — resume hearing others');
    expect(deafenButton.className).toContain('muted');
  });

  it('does not render voice strip when connection state is failed (stale token)', () => {
    voiceState.voiceToken = 'token-stale';
    voiceState.connectedChannelId = 'chan-1';
    voiceState.connectionState = 'failed';
    render(<StatusBar />);
    expect(screen.queryByText('Voice Connected')).not.toBeInTheDocument();
  });

  it('does not render voice strip when connection state is idle', () => {
    voiceState.voiceToken = 'token-leftover';
    voiceState.connectedChannelId = 'chan-1';
    voiceState.connectionState = 'idle';
    render(<StatusBar />);
    expect(screen.queryByText('Voice Connected')).not.toBeInTheDocument();
  });
});
