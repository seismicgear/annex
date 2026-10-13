import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
const api = vi.hoisted(() => ({ peers: vi.fn(), catalog: vi.fn(), preview: vi.fn(), saved: vi.fn(), prepare: vi.fn(), reconcile: vi.fn(), cancel: vi.fn() }));
vi.mock('@/api/channel-authority', () => ({ listAuthorityPeers: api.peers, listAuthorityChannels: api.catalog }));
vi.mock('@/api/authority-reconciliation', () => ({ getReconciliationPreview: api.preview, listReconciliations: api.saved,
  prepareReconciliation: api.prepare, reconcileChannel: api.reconcile, cancelReconciliation: api.cancel }));
import { AuthorityReconciliation } from './AuthorityReconciliation';
const id = 'ee8c9fda-3939-46fa-9083-2dc8d4b4a0dc';
const peer = { peer_id: 7, label: 'Remote', base_url: 'https://remote.example', authority_identity: `annex-server:${'a'.repeat(64)}` };
const channel = { channel_id: 'shared', name: 'Shared', initialized: true, head_seq: 4, head_hash: 'b'.repeat(64), e2e_enabled: false, channel_type: 'Text' };
const preview = { channel_id: 'shared', snapshot_sha256: 'd'.repeat(64), messages: 5, edits: 2, attachments: 3, key_records: 7 };
const intent = { peer_id: 7, authority_identity: peer.authority_identity, device_id: 'c'.repeat(64), operation_id: id, expected_snapshot_sha256: preview.snapshot_sha256 };
const pending = { operation_id: id, status: 'pending', remote_join_accepted: true, after_seq: 2, head_seq: 4, head_hash: 'b'.repeat(64), retained_messages: 0, removed_messages: 0, conflicts: [] };
const complete = { ...pending, status: 'complete', after_seq: 4, retained_messages: 5, removed_messages: 1 };
const completeAction = vi.fn(), busy = vi.fn();
const props = { pseudonym: 'moderator', channelId: 'shared', disabled: false, onBusyChange: busy, onComplete: completeAction };
beforeEach(() => {
  vi.clearAllMocks(); api.peers.mockResolvedValue([peer]); api.catalog.mockResolvedValue({ peer, channels: [channel], next_after_channel_id: null });
  api.preview.mockResolvedValue(preview); api.saved.mockResolvedValue([]); api.prepare.mockResolvedValue(intent);
  api.reconcile.mockResolvedValue(pending); api.cancel.mockResolvedValue({ ...pending, status: 'cancelled' }); completeAction.mockResolvedValue(undefined);
});
afterEach(cleanup);
async function open() {
  const view = render(<AuthorityReconciliation {...props} />);
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Review partner reconciliation' })); });
  return view;
}
async function review() {
  fireEvent.change(screen.getByRole('combobox', { name: 'Reconciliation partner' }), { target: { value: '7' } });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load matching partner channel' })); });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Review retained local history' })); });
}
async function submit() {
  fireEvent.click(screen.getByRole('checkbox', { name: 'I verified this partner fingerprint and local snapshot' }));
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Confirm reconciliation' })); });
}
describe('explicit retained-history reconciliation controls', () => {
  it('requires the displayed exact authority and retained snapshot and lets review cancel without submitting reconciliation', async () => {
    await open(); await review();
    const prompt = screen.getByRole('region', { name: 'Confirm history reconciliation' });
    expect(prompt).toHaveTextContent(peer.authority_identity); expect(prompt).toHaveTextContent(preview.snapshot_sha256);
    expect(prompt).toHaveTextContent('5 messages, 2 edits, 3 attachments, and 7 encryption records');
    expect(screen.getByRole('button', { name: 'Confirm reconciliation' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel reconciliation review' }));
    expect(api.prepare).not.toHaveBeenCalled(); expect(api.reconcile).not.toHaveBeenCalled();
    await review(); await submit();
    expect(api.prepare).toHaveBeenCalledWith('moderator', preview, peer, channel);
    expect(api.reconcile).toHaveBeenCalledWith('moderator', 'shared', intent);
    expect(screen.getByRole('status')).toHaveTextContent('Verified through event 2 of 4');
    expect(completeAction).not.toHaveBeenCalled();
  });
  it('does not turn a lost acknowledgement into a second operation; the same request resumes', async () => {
    api.reconcile.mockRejectedValueOnce(new Error('Connection lost')).mockResolvedValueOnce(complete);
    await open(); await review(); await submit();
    expect(screen.getByRole('alert')).toHaveTextContent('Connection lost');
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Resume reconciliation' })); });
    expect(api.prepare).toHaveBeenCalledTimes(1);
    expect(api.reconcile.mock.calls).toEqual([['moderator', 'shared', intent], ['moderator', 'shared', intent]]);
    expect(completeAction).toHaveBeenCalledTimes(1); expect(busy).toHaveBeenLastCalledWith(false);
    expect(screen.getByRole('status')).toHaveTextContent('Reconciliation completed');
    expect(screen.getByText('5 messages retained; 1 messages removed by authenticated authority history.')).toBeVisible();
  });
  it('restores an exact saved operation after reload but requires explicit verification before resuming', async () => {
    api.saved.mockResolvedValue([{ ...intent, ...pending, channel_id: 'shared' }]);
    await open(); fireEvent.click(screen.getByRole('button', { name: 'Review saved operation' }));
    expect(screen.getByText(intent.device_id)).toBeVisible(); expect(screen.getByText(intent.expected_snapshot_sha256)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Resume reconciliation' })).toBeDisabled();
    expect(api.reconcile).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('checkbox', { name: 'I verified this partner fingerprint and local snapshot' }));
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Resume reconciliation' })); });
    expect(api.reconcile).toHaveBeenCalledWith('moderator', 'shared', expect.objectContaining(intent)); expect(api.prepare).not.toHaveBeenCalled();
  });
  it('shows exact conflict IDs without claiming adoption or loss and resumes only by explicit action', async () => {
    api.reconcile.mockResolvedValue({ ...pending, status: 'conflict', conflicts: [{ kind: 'message', id: 'unmatched-local-message' }, { kind: 'key_epoch', id: 'epoch-7' }] });
    await open(); await review(); await submit();
    const conflicts = screen.getByRole('region', { name: 'Reconciliation conflicts' });
    expect(conflicts).toHaveTextContent('unmatched-local-message'); expect(conflicts).toHaveTextContent('epoch-7');
    expect(screen.getByRole('status')).toHaveTextContent('current local channel state is preserved');
    expect(completeAction).not.toHaveBeenCalled(); expect(api.reconcile).toHaveBeenCalledTimes(1);
    expect(screen.getByRole('button', { name: 'Review a new local snapshot' })).toBeVisible();
  });
  it('cancellation confirms staging only, keeps real failures visible, and preserves an already accepted remote Join', async () => {
    await open(); await review(); await submit();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel remaining reconciliation' }));
    const prompt = screen.getByRole('region', { name: 'Confirm reconciliation cancellation' });
    expect(prompt).toHaveTextContent('cancellation does not reverse that Join');
    fireEvent.click(within(prompt).getByRole('button', { name: 'Keep reconciliation' })); expect(api.cancel).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel remaining reconciliation' }));
    api.cancel.mockRejectedValueOnce(new Error('Server unavailable'));
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Confirm cancellation' })); });
    expect(screen.getByRole('alert')).toHaveTextContent('Server unavailable');
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Confirm cancellation' })); });
    expect(api.cancel.mock.calls).toEqual([['moderator', 'shared', id], ['moderator', 'shared', id]]);
    expect(screen.getByRole('status')).toHaveTextContent('retained channel history was not replaced');
    expect(screen.getByText('The partner has accepted this identity’s channel Join. Cancelling staging does not reverse that Join.')).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Resume reconciliation' })).toBeNull(); expect(completeAction).not.toHaveBeenCalled();
  });
  it('searches later catalog pages for this exact ID and does not permit an uninitialized or different channel', async () => {
    api.catalog.mockResolvedValueOnce({ peer, channels: [{ ...channel, channel_id: 'different' }], next_after_channel_id: 'different' })
      .mockResolvedValueOnce({ peer, channels: [{ ...channel, initialized: false }], next_after_channel_id: null });
    await open(); fireEvent.change(screen.getByRole('combobox', { name: 'Reconciliation partner' }), { target: { value: '7' } });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load matching partner channel' })); });
    expect(screen.queryByRole('button', { name: 'Review retained local history' })).toBeNull();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load more partner channels' })); });
    expect(api.catalog).toHaveBeenLastCalledWith('moderator', peer, 'different');
    expect(screen.getByText('The partner must initialize its channel authority before reconciliation.')).toBeVisible();
    expect(api.preview).not.toHaveBeenCalled(); expect(api.reconcile).not.toHaveBeenCalled();
  });
  it('blocks a reviewed mutation immediately when the live identity loses ownership', async () => {
    const view = await open(); await review();
    fireEvent.click(screen.getByRole('checkbox', { name: 'I verified this partner fingerprint and local snapshot' }));
    view.rerender(<AuthorityReconciliation {...props} disabled />);
    expect(screen.getByRole('button', { name: 'Confirm reconciliation' })).toBeDisabled();
    expect(api.prepare).not.toHaveBeenCalled(); expect(api.reconcile).not.toHaveBeenCalled();
  });
  it('always releases the parent busy state when successful adoption unmounts these controls', async () => {
    api.reconcile.mockResolvedValue(complete);
    const view = await open(); completeAction.mockImplementation(async () => { view.unmount(); });
    await review(); await submit(); expect(completeAction).toHaveBeenCalledTimes(1); expect(busy).toHaveBeenLastCalledWith(false);
  });
});
