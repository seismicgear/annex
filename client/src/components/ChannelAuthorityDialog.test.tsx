import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ChannelAuthorityDialog } from './ChannelAuthorityDialog';
const state = vi.hoisted(() => ({ get: vi.fn(), initialize: vi.fn(), moderator: true, session: 'connected', members: new Set<string>(), reconciliation: vi.fn(), serial: 1, loadChannels: vi.fn(), selectChannel: vi.fn() }));
vi.mock('@/api/core', () => ({ getCredentialContext: () => ({ serial: state.serial, pseudonymId: 'alice' }),
  isCredentialContextCurrent: (context: { serial: number }) => context.serial === state.serial }));
vi.mock('./AuthorityReconciliation', () => ({ AuthorityReconciliation: (...args: unknown[]) => { state.reconciliation(...args); return <p>Reconciliation controls</p>; } }));
vi.mock('@/api/channel-authority', () => ({ getChannelAuthority: (...args: unknown[]) => state.get(...args), initializeChannelAuthority: (...args: unknown[]) => state.initialize(...args) }));
vi.mock('@/stores/identity', () => ({ useIdentityStore: (select: (value: unknown) => unknown) => select({ permissions: { capabilities: { can_moderate: state.moderator } } }) }));
vi.mock('@/stores/channels', () => ({ useChannelsStore: Object.assign(
  (select: (value: unknown) => unknown) => select({ wsSessionState: state.session, joinedChannelIds: state.members }),
  { getState: () => ({ activeChannelId: 'shared', loadChannels: state.loadChannels, selectChannel: state.selectChannel }) },
) }));
const authority = { channel_id: 'shared', authority_identity: 'annex-server:' + 'a'.repeat(64), authority_server: 'https://local.example', local_authority: true, initialized: false, head_seq: 0, head_hash: '' };
const close = vi.fn();
beforeEach(() => { vi.resetAllMocks(); state.serial = 1; state.loadChannels.mockResolvedValue(undefined); state.selectChannel.mockResolvedValue(undefined); state.moderator = true; state.session = 'connected'; state.members = new Set(['shared']); state.get.mockResolvedValue(authority);
  state.initialize.mockResolvedValue({ ...authority, initialized: true, head_seq: 9, head_hash: 'b'.repeat(64) }); });
afterEach(cleanup);
async function open() { render(<ChannelAuthorityDialog pseudonym="alice" channelId="shared" onClose={close} />); await screen.findByText(authority.authority_identity); }
it('explains retained history and requires separate initialization confirmation', async () => {
  await open(); expect(screen.getByRole('dialog')).toHaveTextContent('preserves retained messages and edit history');
  fireEvent.click(screen.getByRole('button', { name: 'Initialize channel authority' }));
  fireEvent.click(screen.getByRole('button', { name: 'Cancel initialization' })); expect(state.initialize).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Initialize channel authority' }));
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Confirm initialization' })); });
  expect(state.initialize).toHaveBeenCalledWith('alice', authority); expect(screen.getByRole('status')).toHaveTextContent('9 events');
  expect(screen.queryByRole('button', { name: 'Initialize channel authority' })).not.toBeInTheDocument();
});
it('lets members inspect the authority but reserves initialization for local moderators', async () => {
  state.moderator = false; await open(); expect(screen.queryByRole('button', { name: 'Initialize channel authority' })).not.toBeInTheDocument();
  expect(screen.getByText('A moderator on the authority server must initialize this channel.')).toBeVisible();
});
it('keeps a failed initialization visible and permits an explicit retry', async () => {
  state.initialize.mockRejectedValueOnce(new Error('Retained history changed. Retry initialization.')); await open();
  fireEvent.click(screen.getByRole('button', { name: 'Initialize channel authority' }));
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Confirm initialization' })); });
  expect(screen.getByRole('alert')).toHaveTextContent('Retained history changed');
  expect(screen.getByRole('button', { name: 'Confirm initialization' })).toBeEnabled();
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Confirm initialization' })); });
  expect(screen.getByRole('status')).toHaveTextContent('9 events');
});
it('offers reconciliation only to a joined moderator before a local authority is initialized', async () => {
  await open(); expect(screen.getByText('Reconciliation controls')).toBeVisible(); cleanup();
  state.members.clear(); await open(); expect(screen.queryByText('Reconciliation controls')).toBeNull(); cleanup();
  state.members.add('shared'); state.moderator = false; await open(); expect(screen.queryByText('Reconciliation controls')).toBeNull(); cleanup();
  state.moderator = true; state.get.mockResolvedValue({ ...authority, initialized: true, head_seq: 1, head_hash: 'b'.repeat(64) });
  await open(); expect(screen.queryByText('Reconciliation controls')).toBeNull(); cleanup();
  state.get.mockResolvedValue({ ...authority, local_authority: false }); await open(); expect(screen.queryByText('Reconciliation controls')).toBeNull();
});
it('refreshes adopted history only for the same live identity and server that confirmed reconciliation', async () => {
  await open();
  const finished = (state.reconciliation.mock.calls.at(-1)![0] as { onComplete: () => Promise<void> }).onComplete;
  await act(async () => { await finished(); });
  expect(state.loadChannels).toHaveBeenCalledWith('alice'); expect(state.selectChannel).toHaveBeenCalledWith('alice', 'shared');
  state.loadChannels.mockClear(); state.selectChannel.mockClear();
  let finishRead!: (value: unknown) => void;
  state.get.mockReturnValueOnce(new Promise(resolve => { finishRead = resolve; }));
  await act(async () => { const pending = finished(); state.serial++; finishRead(authority); await pending; });
  expect(state.loadChannels).not.toHaveBeenCalled(); expect(state.selectChannel).not.toHaveBeenCalled();
});
