import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';
import { MessageInput } from './MessageInput';

let channelsState: {
  activeChannelId: string | null;
  activeChannelKeyState: 'ready' | 'pending' | 'failed' | 'unverified' | 'recovery';
  wsConnected: boolean;
  sendMessage: ReturnType<typeof vi.fn>;
  composerError: string | null;
  clearComposerError: ReturnType<typeof vi.fn>;
  pendingSends: Map<string, unknown>;
  sendTyping: ReturnType<typeof vi.fn>;
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

const mockUploadChatFile = vi.fn(async () => ({ url: '/uploads/file.png' }));
vi.mock('@/lib/api', () => ({
  uploadChatFile: (...a: unknown[]) => mockUploadChatFile(...(a as [])),
}));

describe('MessageInput', () => {
  beforeEach(() => {
    channelsState = {
      activeChannelId: 'chan-1',
      activeChannelKeyState: 'ready',
      wsConnected: true,
      sendMessage: vi.fn(() => 'req-1'),
      composerError: null,
      clearComposerError: vi.fn(),
      pendingSends: new Map(),
      sendTyping: vi.fn(),
    };

    identityState = {
      identity: { pseudonymId: 'p1' },
    };
  });

  it.each(['pending', 'failed', 'unverified', 'recovery'] as const)('preserves a draft and blocks sending until channel encryption is ready (%s)', async status => {
    channelsState.activeChannelKeyState = status;
    const view = render(<MessageInput />);
    const textarea = screen.getByRole('textbox');
    fireEvent.change(textarea, { target: { value: 'wait for verification' } });
    expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
    expect(screen.getByTitle('Upload image, video, or file')).toBeDisabled();
    await act(async () => { fireEvent.submit(document.querySelector('form')!); });
    expect(channelsState.sendMessage).not.toHaveBeenCalled();
    expect(textarea).toHaveValue('wait for verification');
    channelsState.activeChannelKeyState = 'ready';
    view.rerender(<MessageInput />);
    expect(screen.getByRole('button', { name: 'Send' })).toBeEnabled();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Send' })); });
    expect(channelsState.sendMessage).toHaveBeenCalledOnce();
    expect(channelsState.sendMessage).toHaveBeenCalledWith('wait for verification', 'p1');
    expect(textarea).toHaveValue('');
  });

  it('clears content after successful text send', async () => {
    render(<MessageInput />);

    const textarea = screen.getByPlaceholderText('Type a message...');
    fireEvent.change(textarea, { target: { value: 'hello' } });

    const sendBtn = screen.getByRole('button', { name: 'Send' });
    await act(async () => {
      fireEvent.click(sendBtn);
    });

    expect(channelsState.sendMessage).toHaveBeenCalledWith('hello', 'p1');
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

  it('shows composer error when pending send resolves with error', async () => {
    // With optimistic UI, failed messages stay in the message list (not restored to composer).
    // The composer just shows the error banner.
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

    // Simulate the server returning an error
    channelsState.pendingSends = new Map();
    channelsState.composerError = 'Rate limit exceeded';

    await act(async () => {
      rerender(<MessageInput />);
    });

    // Error shown in banner (draft stays in failed message in message list, not restored to composer)
    expect(screen.getByRole('alert')).toHaveTextContent('Rate limit exceeded');
  });

  it('displays composerError banner from store', () => {
    channelsState.composerError = 'Cannot send — not connected to the server.';

    render(<MessageInput />);

    expect(screen.getByText('Cannot send — not connected to the server.')).toBeInTheDocument();
  });

  it('announces an upload failure instead of inserting it silently', async () => {
    // The bar was a bare `<div className="upload-error-bar">`. Every other
    // error surface in the app carries `role="alert"`; without it a screen
    // reader user presses Send, hears nothing, and is left with the
    // attachment still staged and no indication it was refused.
    mockUploadChatFile.mockRejectedValueOnce(new Error('storage unavailable'));

    render(<MessageInput />);

    const fileInput = document.querySelector('input[type="file"]') as HTMLInputElement;
    const file = new File([new Uint8Array([1, 2, 3])], 'a.png', { type: 'image/png' });
    Object.defineProperty(fileInput, 'files', { value: [file], configurable: true });
    await act(async () => {
      fireEvent.change(fileInput);
    });
    // `handleFileSelect` reads the file through FileReader, which resolves on
    // a later task; submitting before the preview exists takes the plain-text
    // path and never uploads at all.
    await screen.findByText(/a\.png/);

    await act(async () => {
      fireEvent.submit(document.querySelector('form') as HTMLFormElement);
    });

    const alerts = screen.getAllByRole('alert');
    expect(alerts.some((a) => a.textContent?.includes('storage unavailable'))).toBe(true);
  });
});
