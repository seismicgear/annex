import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import { ChannelList } from './ChannelList';

let identityState: {
  identity: { pseudonymId: string } | null;
  permissions: { capabilities: { can_moderate: boolean } } | null;
};

let channelsState: {
  channels: Array<{ channel_id: string; channel_type: string; name: string; federation_scope?: string }>;
  activeChannelId: string | null;
  loading: boolean;
  error: string | null;
  loadChannels: ReturnType<typeof vi.fn>;
  selectChannel: ReturnType<typeof vi.fn>;
  joinChannel: ReturnType<typeof vi.fn>;
  leaveChannel: ReturnType<typeof vi.fn>;
};

vi.mock('@/stores/identity', () => ({
  useIdentityStore: (selector: (state: typeof identityState) => unknown) => selector(identityState),
}));

vi.mock('@/stores/channels', () => ({
  useChannelsStore: () => channelsState,
}));

vi.mock('@/lib/invite', () => ({
  createInviteLink: vi.fn(async () => ({ url: 'https://invite.test/abc' })),
}));

vi.mock('@/lib/api', () => ({
  getApiBaseUrl: () => 'http://localhost',
}));

vi.mock('@/components/CreateChannelDialog', () => ({
  CreateChannelDialog: () => null,
}));

describe('ChannelList', () => {
  beforeEach(() => {
    identityState = {
      identity: { pseudonymId: 'p1' },
      permissions: { capabilities: { can_moderate: false } },
    };

    channelsState = {
      channels: [
        { channel_id: 'ch-1', channel_type: 'Text', name: 'general' },
      ],
      activeChannelId: null,
      loading: false,
      error: null,
      loadChannels: vi.fn(async () => {}),
      selectChannel: vi.fn(),
      joinChannel: vi.fn(async () => {}),
      leaveChannel: vi.fn(async () => {}),
    };
  });

  it('surfaces error message when joinChannel rejects', async () => {
    channelsState.joinChannel = vi.fn(async () => {
      throw new Error('Network timeout');
    });

    render(<ChannelList />);

    // Click the join button for the channel
    const joinBtn = screen.getByTitle('Join channel');
    await act(async () => {
      joinBtn.click();
    });

    // The error message should be rendered inline
    expect(screen.getByRole('alert')).toHaveTextContent('Network timeout');
  });

  it('clears error when user dismisses it', async () => {
    channelsState.joinChannel = vi.fn(async () => {
      throw new Error('Server error');
    });

    render(<ChannelList />);

    const joinBtn = screen.getByTitle('Join channel');
    await act(async () => {
      joinBtn.click();
    });

    expect(screen.getByRole('alert')).toHaveTextContent('Server error');

    // Dismiss the error
    const dismissBtn = screen.getByLabelText('Dismiss');
    await act(async () => {
      dismissBtn.click();
    });

    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
