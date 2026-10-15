import { cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ChannelEncryptionBar } from './ChannelEncryptionBar';
import type { VerifiedChannelView } from '@/lib/verified-channel';

const state = vi.hoisted(() => ({
  activeChannelId: 'room', activeChannelE2e: true,
  activeChannelKeyState: 'ready', activeChannelKeyError: null as string | null,
  wsSessionState: 'connected', moderator: true,
  enableChannelE2e: vi.fn(), disableChannelE2e: vi.fn(), retryChannelKey: vi.fn(),
  trustChannel: vi.fn(), recoverChannel: vi.fn(),
  verified: null as VerifiedChannelView | null,
}));
vi.mock('@/lib/verified-channel', () => ({ subscribeVerifiedChannels: () => () => {}, getVerifiedChannel: () => state.verified }));
vi.mock('@/lib/message-crypto', () => ({ getChannelTrustPrompt: () => ({ channelId: 'room', authorityIdentity: 'annex-server:' + 'a'.repeat(64), genesisHash: 'b'.repeat(64) }) }));
vi.mock('@/lib/e2e-store', () => ({ getE2eControls: () => state }));
vi.mock('@/stores/channels', () => ({ useChannelsStore: (select: (value: typeof state) => unknown) => select(state) }));
vi.mock('@/stores/identity', () => ({ useIdentityStore: (select: (value: unknown) => unknown) => select({
  identity: { pseudonymId: 'alice' }, permissions: { capabilities: { can_moderate: state.moderator } },
}) }));
beforeEach(() => {
  vi.clearAllMocks();
  Object.assign(state, { activeChannelId: 'room', activeChannelE2e: true, activeChannelKeyState: 'ready',
    activeChannelKeyError: null, wsSessionState: 'connected', moderator: true });
  state.disableChannelE2e.mockResolvedValue(undefined);
  state.retryChannelKey.mockResolvedValue(undefined);
  state.trustChannel.mockResolvedValue(undefined); state.recoverChannel.mockResolvedValue(undefined);
  state.verified = { authorityIdentity: 'annex-server:' + 'a'.repeat(64), channelId: 'room', genesisHash: 'b'.repeat(64),
    headHash: 'c'.repeat(64), trusted: true, enabled: true, joinPolicy: 'eligible_self_join', members: [{ pseudonym: 'alice', role: 'moderator', devices: [] }] };
});
afterEach(cleanup);

describe('channel encryption moderator controls', () => {
  it('requires explicit first trust, displays the exact fingerprint, and cancel does not pin it', async () => {
    Object.assign(state, { activeChannelE2e: false, activeChannelKeyState: 'unverified', moderator: false });
    render(<ChannelEncryptionBar />);
    expect(state.trustChannel).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Verify encryption' }));
    const dialog = screen.getByRole('dialog', { name: 'Verify encryption' });
    expect(dialog).toHaveTextContent('annex-server:' + 'a'.repeat(64));
    expect(dialog).toHaveTextContent('b'.repeat(64));
    fireEvent.click(within(dialog).getByRole('button', { name: 'Cancel' }));
    expect(state.trustChannel).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Verify encryption' }));
    fireEvent.click(screen.getByRole('button', { name: 'Trust this channel' }));
    await vi.waitFor(() => expect(state.trustChannel).toHaveBeenCalledWith('room', 'annex-server:' + 'a'.repeat(64), 'b'.repeat(64)));
    expect(state.retryChannelKey).toHaveBeenCalledWith('alice');
  });
  it('shows a conflicting history rejection and keeps trust retry available', async () => {
    state.activeChannelKeyState = 'unverified';
    state.trustChannel.mockRejectedValueOnce(new Error('The channel fingerprint changed.'));
    render(<ChannelEncryptionBar />);
    fireEvent.click(screen.getByRole('button', { name: 'Verify encryption' }));
    fireEvent.click(screen.getByRole('button', { name: 'Trust this channel' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('fingerprint changed');
    expect(state.retryChannelKey).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Trust this channel' })).toBeEnabled();
  });
  it('explains device approval versus recovery and only replaces devices after explicit action', async () => {
    state.activeChannelKeyState = 'recovery';
    render(<ChannelEncryptionBar />);
    fireEvent.click(screen.getByRole('button', { name: 'Recover encryption access' }));
    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveTextContent('approve this device under Encryption devices');
    expect(dialog).toHaveTextContent('reading it still requires a saved history key');
    expect(state.recoverChannel).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Recover on this device' }));
    await vi.waitFor(() => expect(state.recoverChannel).toHaveBeenCalledWith('room'));
    expect(state.retryChannelKey).toHaveBeenCalledWith('alice');
  });
  it('requires an explicit disable confirmation and preserves history in its explanation', async () => {
    render(<ChannelEncryptionBar />);
    fireEvent.click(screen.getByRole('button', { name: 'Disable encryption' }));
    expect(state.disableChannelE2e).not.toHaveBeenCalled();
    const dialog = screen.getByRole('dialog', { name: 'Disable channel encryption' });
    expect(dialog).toHaveTextContent('Existing encrypted messages and their history keys will be preserved');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Disable encryption' }));
    expect(state.disableChannelE2e).toHaveBeenCalledWith('alice');
    await vi.waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
  });
  it('canceling does not change the channel', () => {
    render(<ChannelEncryptionBar />);
    fireEvent.click(screen.getByRole('button', { name: 'Disable encryption' }));
    fireEvent.click(screen.getByRole('button', { name: 'Keep encryption' }));
    expect(state.disableChannelE2e).not.toHaveBeenCalled();
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  });
  it('keeps a rejected disable visible and permits another ordinary attempt', async () => {
    state.disableChannelE2e.mockRejectedValueOnce(new Error('Moderator permission was revoked.'));
    render(<ChannelEncryptionBar />);
    fireEvent.click(screen.getByRole('button', { name: 'Disable encryption' }));
    const dialog = screen.getByRole('dialog');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Disable encryption' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Moderator permission was revoked.');
    fireEvent.click(within(dialog).getByRole('button', { name: 'Disable encryption' }));
    await vi.waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(state.disableChannelE2e).toHaveBeenCalledTimes(2);
  });
  it('does not offer moderator changes to an ordinary member', () => {
    state.moderator = false;
    state.verified!.members[0].role = 'member';
    render(<ChannelEncryptionBar />);
    expect(screen.queryByRole('button', { name: 'Disable encryption' })).not.toBeInTheDocument();
  });
  it('uses the verified channel role even when it differs from global administration', () => {
    state.moderator = false;
    const view = render(<ChannelEncryptionBar />);
    expect(screen.getByRole('button', { name: 'Disable encryption' })).toBeEnabled();
    state.moderator = true;
    state.verified = { ...state.verified!, members: [{ pseudonym: 'alice', role: 'member', devices: [] }] };
    view.rerender(<ChannelEncryptionBar />);
    expect(screen.queryByRole('button', { name: 'Disable encryption' })).not.toBeInTheDocument();
  });
  it('permits the server moderator to create only a genuinely absent genesis', () => {
    Object.assign(state, { activeChannelE2e: false, activeChannelKeyState: 'ready' });
    state.verified = { ...state.verified!, genesisHash: null, headHash: null, trusted: false, enabled: false, members: [] };
    render(<ChannelEncryptionBar />);
    expect(screen.getByRole('button', { name: /Enable encryption/ })).toBeEnabled();
  });
  it('blocks encryption mutations on the displaced device', () => {
    state.wsSessionState = 'taken_over';
    state.activeChannelKeyState = 'pending';
    render(<ChannelEncryptionBar />);
    expect(screen.getByRole('button', { name: 'Disable encryption' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Check again' })).toBeDisabled();
  });
  it('reports a key retry rejection rather than leaving an unhandled promise', async () => {
    state.activeChannelKeyState = 'failed';
    state.retryChannelKey.mockRejectedValueOnce(new Error('The server is unavailable.'));
    render(<ChannelEncryptionBar />);
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
    await vi.waitFor(() => expect(screen.getAllByRole('alert').some(element => element.textContent?.includes('server is unavailable'))).toBe(true));
  });
});
