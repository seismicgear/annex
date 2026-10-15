import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
const calls = vi.hoisted(() => ({ read: vi.fn(), retry: vi.fn(), create: vi.fn(), revoke: vi.fn(), serial: 1,
  session: 'connected', transfer: undefined as undefined | { status: string; transferredBytes: number; totalBytes: number }, listener: () => {} }));
vi.mock('@/api/federated-attachments', () => ({
  parseOrdinaryAttachment: (path: string) => ({ id: 'id', filename: path.split('/').at(-1), path }),
  readOrdinaryFederatedAttachment: calls.read, retryOrdinaryFederatedAttachment: calls.retry,
}));
vi.mock('@/api/core', () => ({ getCredentialContext: () => ({ serial: calls.serial }) }));
vi.mock('@/lib/attachment-transfers', () => ({ getAttachmentTransfer: () => calls.transfer,
  subscribeAttachmentTransfers: (listener: () => void) => { calls.listener = listener; return () => {}; },
}));
vi.mock('@/stores/channels', () => ({ useChannelsStore: (select: (state: unknown) => unknown) => select({ wsSessionState: calls.session }) }));
import { FederatedAttachment } from './FederatedAttachment';
const props = { pseudonym: 'reader', channelId: 'channel', path: '/uploads/chat/files/id.txt', category: 'file' as const };
beforeEach(() => {
  vi.clearAllMocks(); calls.serial = 1; calls.session = 'connected'; calls.transfer = undefined;
  calls.read.mockResolvedValue(new Blob(['ordinary'])); calls.retry.mockResolvedValue(undefined);
  calls.create.mockReturnValue('blob:local-file');
  Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: calls.create });
  Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: calls.revoke });
});
afterEach(cleanup);
describe('ordinary federated attachment presentation', () => {
  it('downloads only the authorized local blob and cleans up its object URL', async () => {
    const view = render(<FederatedAttachment {...props} />);
    const download = await screen.findByRole('link', { name: 'Download id.txt' });
    expect(download.getAttribute('href')).toBe('blob:local-file'); expect(download.getAttribute('download')).toBe('id.txt');
    view.unmount(); expect(calls.revoke).toHaveBeenCalledWith('blob:local-file');
  });
  it('shows actual pending progress, invokes a real retry operation, then reads again', async () => {
    calls.transfer = { status: 'pending', transferredBytes: 3, totalBytes: 7 }; calls.read.mockRejectedValueOnce(new Error('Not ready'));
    render(<FederatedAttachment {...props} />);
    const retry = await screen.findByRole('button', { name: 'Retry attachment' });
    expect(screen.getByRole('alert').textContent).toBe('Not ready');
    fireEvent.click(retry);
    expect(screen.getByText('Attachment transfer pending (3 of 7 bytes).')).toBeTruthy();
    await screen.findByRole('link', { name: 'Download id.txt' });
    expect(calls.retry).toHaveBeenCalledWith('reader', 'channel', 'id'); expect(calls.read).toHaveBeenCalledTimes(2);
  });
  it('keeps denied reads visible alongside a pending hint and renders even empty native errors honestly', async () => {
    calls.transfer = { status: 'pending', transferredBytes: 3, totalBytes: 7 };
    calls.read.mockRejectedValueOnce(new Error('Membership was revoked'));
    calls.retry.mockRejectedValueOnce(new DOMException('', 'OperationError'));
    render(<FederatedAttachment {...props} />);
    expect((await screen.findByRole('alert')).textContent).toBe('Membership was revoked');
    expect(screen.getByRole('status').textContent).toContain('3 of 7');
    fireEvent.click(screen.getByRole('button', { name: 'Retry attachment' }));
    expect((await screen.findByRole('alert')).textContent).toBe('The attachment retry failed.');
  });
  it('recovers a failed read when the actual ready hint arrives without another user action', async () => {
    calls.read.mockRejectedValueOnce(new Error('Not ready'));
    render(<FederatedAttachment {...props} />); await screen.findByRole('button', { name: 'Retry attachment' });
    await act(async () => { calls.transfer = { status: 'ready', transferredBytes: 7, totalBytes: 7 }; calls.listener(); });
    await screen.findByRole('link', { name: 'Download id.txt' }); expect(calls.retry).not.toHaveBeenCalled();
  });
  it('removal destroys an already loaded URL, closes its preview and offers no retry', async () => {
    render(<FederatedAttachment {...props} category="image" path="/uploads/chat/images/id.png" />);
    fireEvent.click(await screen.findByRole('button', { name: 'View image full size' }));
    expect(screen.getByRole('dialog', { name: 'Full size attachment' })).toBeTruthy();
    await act(async () => { calls.transfer = { status: 'removed', transferredBytes: 7, totalBytes: 7 }; calls.listener(); });
    expect(screen.queryByRole('link')).toBeNull(); expect(screen.queryByRole('dialog')).toBeNull();
    expect(screen.getByText('This attachment was removed.')).toBeTruthy(); expect(screen.queryByRole('button', { name: 'Retry attachment' })).toBeNull();
    expect(calls.revoke).toHaveBeenCalledWith('blob:local-file');
  });
  it('does not display a stale read after unmount and blocks retry on takeover', async () => {
    calls.read.mockRejectedValueOnce(new Error('Offline')); calls.session = 'taken_over';
    const view = render(<FederatedAttachment {...props} />);
    expect((await screen.findByRole('button', { name: 'Retry attachment' }) as HTMLButtonElement).disabled).toBe(true);
    view.unmount();
    let finish!: (blob: Blob) => void; calls.read.mockReturnValue(new Promise(resolve => { finish = resolve; }));
    const next = render(<FederatedAttachment {...props} />); next.unmount();
    await act(async () => { finish(new Blob(['late'])); }); expect(calls.create).not.toHaveBeenCalled();
  });
});
