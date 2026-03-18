import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { MessageInput } from './MessageInput';

let channelsState: {
  activeChannelId: string | null;
  wsConnected: boolean;
  sendMessage: ReturnType<typeof vi.fn>;
  composerError: string | null;
  clearComposerError: ReturnType<typeof vi.fn>;
};

let identityState: {
  identity: { pseudonymId: string } | null;
};

vi.mock('@/stores/channels', () => ({
  useChannelsStore: Object.assign(
    (selector?: (state: typeof channelsState) => unknown) =>
      selector ? selector(channelsState) : channelsState,
    {
      getState: () => channelsState,
    },
  ),
}));

vi.mock('@/stores/identity', () => ({
  useIdentityStore: (selector: (state: typeof identityState) => unknown) => selector(identityState),
}));

vi.mock('@/lib/api', () => ({
  uploadChatFile: vi.fn(async () => ({ url: '/uploads/file.png' })),
}));

describe('MessageInput', () => {
  beforeEach(() => {
    channelsState = {
      activeChannelId: 'chan-1',
      wsConnected: true,
      sendMessage: vi.fn(() => true),
      composerError: null,
      clearComposerError: vi.fn(),
    };

    identityState = {
      identity: { pseudonymId: 'p1' },
    };
  });

  it('clears content after successful text send', async () => {
    render(<MessageInput />);

    const textarea = screen.getByPlaceholderText('Type a message...');
    fireEvent.change(textarea, { target: { value: 'hello' } });

    const sendBtn = screen.getByRole('button', { name: 'Send' });
    await act(async () => {
      fireEvent.click(sendBtn);
    });

    expect(channelsState.sendMessage).toHaveBeenCalledWith('hello');
    expect(textarea).toHaveValue('');
  });

  it('keeps content when sendMessage returns false (socket unavailable)', async () => {
    channelsState.sendMessage = vi.fn(() => false);

    render(<MessageInput />);

    const textarea = screen.getByPlaceholderText('Type a message...');
    fireEvent.change(textarea, { target: { value: 'lost message' } });

    const sendBtn = screen.getByRole('button', { name: 'Send' });
    await act(async () => {
      fireEvent.click(sendBtn);
    });

    // Content should still be in the textarea
    expect(textarea).toHaveValue('lost message');
  });

  it('keeps content when synchronous send sets composerError', async () => {
    channelsState.sendMessage = vi.fn(() => {
      channelsState.composerError = 'WebSocket is not connected';
      return true;
    });

    render(<MessageInput />);

    const textarea = screen.getByPlaceholderText('Type a message...');
    fireEvent.change(textarea, { target: { value: 'retry me' } });

    const sendBtn = screen.getByRole('button', { name: 'Send' });
    await act(async () => {
      fireEvent.click(sendBtn);
    });

    // Content should remain because composerError was set
    expect(textarea).toHaveValue('retry me');
  });

  it('displays composerError banner from store', () => {
    channelsState.composerError = 'Cannot send — not connected to the server.';

    render(<MessageInput />);

    expect(screen.getByText('Cannot send — not connected to the server.')).toBeInTheDocument();
  });
});
