import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { FederatedChannelsDialog } from './FederatedChannelsDialog';
const calls = vi.hoisted(() => ({ peers: vi.fn(), catalog: vi.fn(), attach: vi.fn(), authority: vi.fn(), load: vi.fn(), select: vi.fn(),
  session: 'connected', channels: [] as { channel_id: string }[], joined: new Set<string>(), error: null as string | null }));
vi.mock('@/api/channel-authority', () => ({ listAuthorityPeers: (...a: unknown[]) => calls.peers(...a), listAuthorityChannels: (...a: unknown[]) => calls.catalog(...a),
  attachAuthorityChannel: (...a: unknown[]) => calls.attach(...a), getChannelAuthority: (...a: unknown[]) => calls.authority(...a) }));
vi.mock('@/stores/channels', () => ({ useChannelsStore: Object.assign((select: (state: unknown) => unknown) => select({ wsSessionState: calls.session }), {
  getState: () => ({ channels: calls.channels, joinedChannelIds: calls.joined, loadChannels: calls.load, selectChannel: calls.select, error: calls.error }),
}) }));
const peer = { peer_id: 7, label: 'Partner', base_url: 'https://partner.example', authority_identity: 'annex-server:' + 'a'.repeat(64) };
const channel = { channel_id: 'shared', name: 'Shared room', channel_type: 'Text', e2e_enabled: true, initialized: true, head_seq: 3, head_hash: 'b'.repeat(64) };
const close = vi.fn();
beforeEach(() => { vi.resetAllMocks(); calls.session = 'connected'; calls.channels = []; calls.joined = new Set(['shared']); calls.error = null;
  calls.peers.mockResolvedValue([peer]); calls.catalog.mockResolvedValue({ peer, channels: [channel], next_after_channel_id: null }); calls.attach.mockResolvedValue(undefined); });
afterEach(cleanup);
async function openCatalog() {
  render(<FederatedChannelsDialog pseudonym="alice" onClose={close} />);
  await screen.findByRole('option', { name: /Partner —/ });
  fireEvent.change(screen.getByLabelText('Partner server'), { target: { value: '7' } });
  await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load partner channels' })); });
}
describe('ordinary federation attachment controls', () => {
  it('shows the pinned fingerprint and requires explicit confirmation; cancel submits nothing', async () => {
    await openCatalog(); fireEvent.click(screen.getByRole('button', { name: 'Join channel' }));
    const confirm = screen.getByRole('region', { name: 'Confirm federated channel' });
    expect(confirm).toHaveTextContent(peer.authority_identity); expect(confirm).toHaveTextContent('shared');
    expect(screen.getByRole('button', { name: 'Confirm joining' })).toBeDisabled();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel joining' })); expect(calls.attach).not.toHaveBeenCalled();
    expect(screen.queryByRole('region', { name: 'Confirm federated channel' })).not.toBeInTheDocument();
  });
  it('opens only the actual accepted and reloaded membership', async () => {
    await openCatalog(); fireEvent.click(screen.getByRole('button', { name: 'Join channel' })); fireEvent.click(screen.getByLabelText('I verified this partner fingerprint'));
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Confirm joining' })); });
    expect(calls.attach).toHaveBeenCalledWith('alice', peer, channel); expect(calls.load).toHaveBeenCalledWith('alice');
    expect(calls.select).toHaveBeenCalledWith('alice', 'shared'); expect(close).toHaveBeenCalledOnce();
  });
  it('preserves local history on a colliding ID and never calls attachment', async () => {
    calls.channels = [{ channel_id: 'shared' }]; calls.authority.mockResolvedValue({ local_authority: true, authority_identity: 'annex-server:' + 'd'.repeat(64) });
    await openCatalog(); fireEvent.click(screen.getByRole('button', { name: 'Join channel' })); fireEvent.click(screen.getByLabelText('I verified this partner fingerprint'));
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Confirm joining' })); });
    expect(screen.getByRole('alert')).toHaveTextContent('history cannot be overwritten'); expect(calls.attach).not.toHaveBeenCalled(); expect(close).not.toHaveBeenCalled();
  });
  it('does not claim success when the accepted membership cannot be loaded', async () => {
    calls.joined.clear(); await openCatalog(); fireEvent.click(screen.getByRole('button', { name: 'Join channel' })); fireEvent.click(screen.getByLabelText('I verified this partner fingerprint'));
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Confirm joining' })); });
    expect(screen.getByRole('alert')).toHaveTextContent('membership has not been loaded'); expect(close).not.toHaveBeenCalled(); expect(calls.select).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Confirm joining' })).toBeEnabled();
  });
  it('offers retry after a real discovery rejection and no join for uninitialized entries', async () => {
    calls.catalog.mockRejectedValueOnce(new Error('The signed partner response expired.'));
    await openCatalog(); expect(screen.getByRole('alert')).toHaveTextContent('signed partner response expired');
    calls.catalog.mockResolvedValueOnce({ peer, channels: [{ ...channel, initialized: false }], next_after_channel_id: null });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load partner channels' })); });
    expect(screen.queryByRole('button', { name: 'Join channel' })).not.toBeInTheDocument();
    expect(screen.getByText(/must initialize this channel/)).toBeVisible();
  });
  it('disables final mutation if this live identity is taken over', async () => {
    await openCatalog(); fireEvent.click(screen.getByRole('button', { name: 'Join channel' }));
    const confirm = screen.getByRole('region', { name: 'Confirm federated channel' });
    expect(within(confirm).getByRole('button', { name: 'Cancel joining' })).toBeEnabled();
    calls.session = 'taken_over'; fireEvent.click(screen.getByLabelText('I verified this partner fingerprint'));
    expect(screen.getByRole('button', { name: 'Confirm joining' })).toBeDisabled(); expect(calls.attach).not.toHaveBeenCalled();
  });
  it('ignores a prior partner’s late channel page after the selection changes', async () => {
    const second = { ...peer, peer_id: 8, label: 'Second partner', base_url: 'https://second.example', authority_identity: 'annex-server:' + 'c'.repeat(64) };
    calls.peers.mockResolvedValueOnce([peer, second]);
    let release!: (value: unknown) => void; calls.catalog.mockImplementationOnce(() => new Promise(resolve => { release = resolve; }));
    render(<FederatedChannelsDialog pseudonym="alice" onClose={close} />);
    await screen.findByRole('option', { name: /Second partner —/ });
    fireEvent.change(screen.getByLabelText('Partner server'), { target: { value: '7' } });
    fireEvent.click(screen.getByRole('button', { name: 'Load partner channels' }));
    fireEvent.change(screen.getByLabelText('Partner server'), { target: { value: '8' } });
    await act(async () => { release({ peer, channels: [channel], next_after_channel_id: null }); });
    expect(screen.queryByText('Shared room')).not.toBeInTheDocument();
    expect(screen.getByRole('dialog')).toHaveTextContent(second.authority_identity);
    expect(screen.getByRole('button', { name: 'Load partner channels' })).toBeEnabled();
  });
  it('loads the explicit next page without replacing the first page', async () => {
    calls.catalog.mockResolvedValueOnce({ peer, channels: [channel], next_after_channel_id: 'shared' });
    await openCatalog();
    calls.catalog.mockResolvedValueOnce({ peer, channels: [{ ...channel, channel_id: 'second', name: 'Second room' }], next_after_channel_id: null });
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Load more channels' })); });
    expect(calls.catalog).toHaveBeenLastCalledWith('alice', peer, 'shared');
    expect(screen.getByText('Shared room')).toBeVisible(); expect(screen.getByText('Second room')).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Load more channels' })).not.toBeInTheDocument();
  });
});
