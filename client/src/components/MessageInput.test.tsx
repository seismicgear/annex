import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { MessageInput } from './MessageInput';

let channelsState: {
  activeChannelId: string | null;
  wsConnected: boolean;
  sendMessage: ReturnType<typeof vi.fn>;
  composerError: string | null;
  clearComposerError: ReturnType<typeof vi.fn>;
  pendingSends: Map<string, unknown>;
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
      sendMessage: vi.fn(() => 'req-1'),
      composerError: null,
      clearComposerError: vi.fn(),
      pendingSends: new Map(),
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

  it('keeps content when sendMessage returns null (socket unavailable)', async () => {
    channelsState.sendMessage = vi.fn(() => null);

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

  it('restores draft when async error resolves the pending send', async () => {
    // sendMessage returns a request ID and adds it to pendingSends
    const reqId = 'req-err-1';
    channelsState.pendingSends = new Map([[reqId, { clientRequestId: reqId, content: 'retry me', sentAt: Date.now() }]]);
    channelsState.sendMessage = vi.fn(() => reqId);

    const { rerender } = render(<MessageInput />);

    const textarea = screen.getByPlaceholderText('Type a message...');
    fireEvent.change(textarea, { target: { value: 'retry me' } });

    const sendBtn = screen.getByRole('button', { name: 'Send' });
    await act(async () => {
      fireEvent.click(sendBtn);
    });

    // Composer should be optimistically cleared
    expect(textarea).toHaveValue('');

    // Simulate the server returning an error — pending send removed, composerError set
    channelsState.pendingSends = new Map();
    channelsState.composerError = 'Rate limit exceeded';

    await act(async () => {
      rerender(<MessageInput />);
    });

    // Draft should be restored and error shown
    expect(textarea).toHaveValue('retry me');
    expect(screen.getByRole('alert')).toHaveTextContent('Rate limit exceeded');
  });

  it('displays composerError banner from store', () => {
    channelsState.composerError = 'Cannot send — not connected to the server.';

    render(<MessageInput />);

    expect(screen.getByText('Cannot send — not connected to the server.')).toBeInTheDocument();
  });
});
