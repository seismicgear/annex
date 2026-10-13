import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { EncryptedAttachment } from './EncryptedAttachment';
import { receiveAttachmentTransfer } from '@/lib/attachment-transfers';
import type { PrivateAttachment } from '@/lib/encrypted-attachments';
const calls = vi.hoisted(() => ({ read: vi.fn(), retry: vi.fn(), session: 'connected' }));
vi.mock('@/api/encrypted-uploads', () => ({ readEncryptedAttachment: (...args: unknown[]) => calls.read(...args), retryEncryptedAttachment: (...args: unknown[]) => calls.retry(...args) }));
vi.mock('@/stores/channels', () => ({ useChannelsStore: (select: (s: unknown) => unknown) => select({ wsSessionState: calls.session }) }));
const attachment: PrivateAttachment = { id: '11111111-1111-4111-8111-111111111111', url: '/uploads/chat/files/11111111-1111-4111-8111-111111111111.bin',
  name: 'private.txt', contentType: 'text/plain', category: 'file', bytes: 4, plaintextSha256: '1'.repeat(64), ciphertextSha256: '2'.repeat(64), fileKeyHex: '3'.repeat(64),
  epoch: 1, keyId: '4'.repeat(64), authorityNamespace: 'annex-server:' + '5'.repeat(64), channelId: 'room', originIdentity: 'annex-server:' + '5'.repeat(64), originatingServer: 'https://annex.example' };
beforeEach(() => {
  vi.clearAllMocks(); calls.session = 'connected'; calls.read.mockResolvedValue(new Blob(['text'])); calls.retry.mockResolvedValue(undefined);
  vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: vi.fn(() => 'blob:private-test'), revokeObjectURL: vi.fn() }));
});
afterEach(() => { cleanup(); vi.unstubAllGlobals(); });
describe('encrypted attachment presentation', () => {
  it('renders only the verified blob and revokes its URL when the message leaves the view', async () => {
    const view = render(<EncryptedAttachment attachment={attachment} channelId="room" pseudonym="alice" />);
    expect(await screen.findByRole('link', { name: 'Download private.txt' })).toHaveAttribute('href', 'blob:private-test');
    expect(screen.queryByText(attachment.fileKeyHex)).not.toBeInTheDocument();
    const signal = calls.read.mock.calls[0][3] as AbortSignal;
    view.unmount(); expect(signal.aborted).toBe(true); expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:private-test');
  });
  it('uses the current-member retry operation and then fetches again after a failure', async () => {
    calls.read.mockRejectedValueOnce(new Error('Transfer not ready.'));
    render(<EncryptedAttachment attachment={attachment} channelId="room" pseudonym="alice" />);
    expect(await screen.findByRole('alert')).toHaveTextContent('Transfer not ready');
    fireEvent.click(screen.getByRole('button', { name: 'Retry attachment' }));
    expect(await screen.findByRole('link')).toBeVisible();
    expect(calls.retry).toHaveBeenCalledWith('alice', 'room', attachment.id);
    expect(calls.read).toHaveBeenCalledTimes(2);
  });
  it('recovers a transient read failure when the actual attachment ready hint arrives', async () => {
    calls.read.mockRejectedValueOnce(new Error('Transfer not ready.'));
    render(<EncryptedAttachment attachment={attachment} channelId="room" pseudonym="alice" />);
    await screen.findByRole('alert');
    await act(async () => receiveAttachmentTransfer({ type: 'attachment_transfer_changed', channelId: 'room', attachmentId: attachment.id,
      status: 'ready', transferredBytes: 32, totalBytes: 32 }));
    expect(await screen.findByRole('link')).toBeVisible();
    expect(calls.read).toHaveBeenCalledTimes(2);
  });
  it('does not let a pending hint hide a real permission failure or an empty native retry error', async () => {
    const value = { ...attachment, id: '22222222-2222-4222-8222-222222222222', url: '/uploads/chat/files/22222222-2222-4222-8222-222222222222.bin' };
    receiveAttachmentTransfer({ type: 'attachment_transfer_changed', channelId: 'room', attachmentId: value.id,
      status: 'pending', transferredBytes: 0, totalBytes: 32 });
    calls.read.mockRejectedValueOnce(new Error('Membership was revoked'));
    calls.retry.mockRejectedValueOnce(new DOMException('', 'OperationError'));
    render(<EncryptedAttachment attachment={value} channelId="room" pseudonym="alice" />);
    expect(await screen.findByRole('alert')).toHaveTextContent('Membership was revoked');
    expect(screen.getByRole('status')).toHaveTextContent('0 of 32');
    fireEvent.click(screen.getByRole('button', { name: 'Retry attachment' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('The attachment retry failed.');
  });
  it('ignores a failed retry belonging to the attachment that has already left the view', async () => {
    let fail!: (reason: Error) => void;
    calls.read.mockRejectedValueOnce(new Error('Temporarily unavailable'));
    calls.retry.mockReturnValueOnce(new Promise((_resolve, reject) => { fail = reject; }));
    const view = render(<EncryptedAttachment attachment={attachment} channelId="room" pseudonym="alice" />);
    fireEvent.click(await screen.findByRole('button', { name: 'Retry attachment' }));
    const next = { ...attachment, id: '33333333-3333-4333-8333-333333333333', url: '/uploads/chat/files/33333333-3333-4333-8333-333333333333.bin', name: 'next.txt' };
    view.rerender(<EncryptedAttachment attachment={next} channelId="room" pseudonym="alice" />);
    await screen.findByRole('link', { name: 'Download next.txt' });
    await act(async () => { fail(new Error('Old attachment failed')); });
    expect(screen.getByRole('link', { name: 'Download next.txt' })).toBeVisible(); expect(screen.queryByRole('alert')).toBeNull();
  });
});
