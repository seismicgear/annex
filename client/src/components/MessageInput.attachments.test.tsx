import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { MessageInput } from './MessageInput';
import { parsePrivateMessage } from '@/lib/encrypted-attachments';
const state = vi.hoisted(() => ({ activeChannelId: 'room', activeChannelKeyState: 'ready', wsConnected: true, composerError: null,
  sendMessage: vi.fn(), sendTyping: vi.fn(), clearComposerError: vi.fn(), setReplyTo: vi.fn(), replyToMessage: null }));
const calls = vi.hoisted(() => ({ encrypted: vi.fn(), ordinary: vi.fn(), abort: vi.fn(), enabled: true }));
vi.mock('@/stores/channels', () => ({ useChannelsStore: Object.assign((select?: (s: typeof state) => unknown) => select ? select(state) : state, { getState: () => state }) }));
vi.mock('@/stores/identity', () => ({ useIdentityStore: (select: (s: unknown) => unknown) => select({ identity: { pseudonymId: 'alice' } }) }));
vi.mock('@/stores/usernames', () => ({ useUsernameStore: (select: (s: unknown) => unknown) => select({ getDisplayName: () => null }) }));
vi.mock('@/lib/api', () => ({ uploadChatFile: (...args: unknown[]) => calls.ordinary(...args) }));
vi.mock('@/api/encrypted-uploads', () => ({ uploadEncryptedAttachment: (...args: unknown[]) => calls.encrypted(...args), abortStagedUpload: (...args: unknown[]) => calls.abort(...args) }));
vi.mock('@/lib/message-crypto', () => ({ isChannelE2e: () => calls.enabled, isChannelE2eUnknown: () => false }));
const descriptor = { id: '11111111-1111-4111-8111-111111111111', url: '/uploads/chat/files/11111111-1111-4111-8111-111111111111.bin',
  name: 'private.txt', contentType: 'text/plain', category: 'file', bytes: 4, plaintextSha256: '1'.repeat(64), ciphertextSha256: '2'.repeat(64),
  fileKeyHex: '3'.repeat(64), epoch: 1, keyId: '4'.repeat(64), authorityNamespace: 'annex-server:' + '5'.repeat(64), channelId: 'room',
  originIdentity: 'annex-server:' + '5'.repeat(64), originatingServer: 'https://annex.example' };
beforeEach(() => {
  vi.clearAllMocks(); state.activeChannelId = 'room'; state.sendMessage.mockReturnValue('request');
  calls.enabled = true; calls.encrypted.mockResolvedValue(descriptor); calls.abort.mockResolvedValue(undefined);
});
afterEach(cleanup);
function choose() {
  fireEvent.change(document.querySelector('input[type=file]')!, { target: { files: [new File(['text'], 'private.txt', { type: 'text/plain' })] } });
}
describe('attachment composer ownership', () => {
  it('passes private descriptors and opaque IDs to the encrypted send path without a plaintext upload', async () => {
    render(<MessageInput />); choose();
    fireEvent.change(screen.getByRole('textbox'), { target: { value: 'caption' } });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Send' })); });
    expect(calls.ordinary).not.toHaveBeenCalled();
    const [body, identity, reply, ids] = state.sendMessage.mock.calls[0];
    expect(parsePrivateMessage(body)).toEqual({ text: 'caption', attachments: [descriptor] });
    expect([identity, reply, ids]).toEqual(['alice', undefined, [descriptor.id]]);
    expect(calls.abort).not.toHaveBeenCalled();
  });
  it('aborts the unpublished attachment when sending is refused and retains the selected file', async () => {
    state.sendMessage.mockReturnValue(null);
    render(<MessageInput />); choose();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Send' })); });
    expect(calls.abort).toHaveBeenCalledWith('alice', 'room', descriptor.id);
    expect(screen.getByText('private.txt', { exact: false })).toBeVisible();
  });
  it('does not send a late upload into a newly selected channel and cancels its original signal', async () => {
    let release!: (value: unknown) => void;
    calls.encrypted.mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
    const view = render(<MessageInput />); choose();
    fireEvent.click(screen.getByRole('button', { name: 'Send' }));
    const signal = calls.encrypted.mock.calls[0][3] as AbortSignal;
    state.activeChannelId = 'another'; view.rerender(<MessageInput />);
    expect(signal.aborted).toBe(true);
    await act(async () => { release(descriptor); });
    expect(state.sendMessage).not.toHaveBeenCalled();
    expect(calls.abort).toHaveBeenCalledWith('alice', 'room', descriptor.id);
  });
});
