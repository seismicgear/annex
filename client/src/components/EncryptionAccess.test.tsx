import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { EncryptionAccess } from './EncryptionAccess';
import type { VerifiedChannelView } from '@/lib/verified-channel';

const state = vi.hoisted(() => ({
  view: null as VerifiedChannelView | null, wsSessionState: 'connected', retryChannelKey: vi.fn(),
  inspectChannel: vi.fn(), setAdmissionPolicy: vi.fn(), admissionDevices: vi.fn(), admitChannelMember: vi.fn(),
  setChannelRole: vi.fn(), removeChannelMember: vi.fn(),
}));
vi.mock('@/lib/e2e-store', () => ({ getE2eControls: () => state }));
vi.mock('@/lib/verified-channel', () => ({ getVerifiedChannel: () => state.view, subscribeVerifiedChannels: () => () => {} }));
vi.mock('@/stores/channels', () => ({ useChannelsStore: (select: (value: typeof state) => unknown) => select(state) }));
const founder = 'a'.repeat(64), peer = 'b'.repeat(64), device = 'c'.repeat(64);
const close = vi.fn();
beforeEach(() => {
  vi.resetAllMocks(); state.wsSessionState = 'connected';
  state.view = { authorityIdentity: 'annex-server:' + 'd'.repeat(64), channelId: 'room', genesisHash: 'e'.repeat(64), headHash: 'f'.repeat(64),
    trusted: true, enabled: true, joinPolicy: 'eligible_self_join', members: [
      { pseudonym: founder, role: 'moderator', devices: [device] }, { pseudonym: peer, role: 'member', devices: ['1'.repeat(64)] },
    ] };
});
afterEach(cleanup);
const mount = (pseudonym = founder) => render(<EncryptionAccess channelId="room" pseudonym={pseudonym} onClose={close} />);
const memberRow = (pseudonym: string) => document.querySelector(`[data-encryption-member="${pseudonym}"]`) as HTMLElement;

describe('verified encrypted channel access controls', () => {
  it('lets an ordinary member compare trusted fingerprints without exposing moderator controls', () => {
    mount(peer);
    const dialog = screen.getByRole('dialog', { name: 'Encryption access' });
    expect(dialog).toHaveTextContent('annex-server:' + 'd'.repeat(64));
    expect(dialog).toHaveTextContent('e'.repeat(64));
    expect(screen.getByLabelText('Admission policy')).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Make moderator' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Remove member' })).not.toBeInTheDocument();
    expect(screen.queryByText('Admit an identity')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Done' })); expect(close).toHaveBeenCalledOnce();
  });
  it('never presents untrusted roster permissions as authority to manage the channel', () => {
    state.view!.trusted = false; mount();
    expect(screen.getByRole('status')).toHaveTextContent('Verify this channel');
    expect(screen.queryByLabelText('Admission policy')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Admit member' })).not.toBeInTheDocument();
  });
  it('submits an explicit admission policy change and reloads verified key state', async () => {
    mount();
    fireEvent.change(screen.getByLabelText('Admission policy'), { target: { value: 'moderator_admission' } });
    expect(state.setAdmissionPolicy).not.toHaveBeenCalled();
    await act(async () => { fireEvent.click(screen.getByRole('button', { name: 'Save admission policy' })); });
    await vi.waitFor(() => expect(state.setAdmissionPolicy).toHaveBeenCalledWith('room', 'moderator_admission'));
    expect(state.inspectChannel).toHaveBeenCalledWith('room');
    expect(state.retryChannelKey).toHaveBeenCalledWith(founder);
  });
  it('requires a separate confirmation for role changes and cancellation sends nothing', async () => {
    mount();
    fireEvent.click(within(memberRow(peer)).getByRole('button', { name: 'Make moderator' }));
    expect(screen.getByRole('region', { name: 'Confirm encryption access change' })).toHaveTextContent(peer);
    expect(state.setChannelRole).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Cancel change' }));
    expect(state.setChannelRole).not.toHaveBeenCalled();
    fireEvent.click(within(memberRow(peer)).getByRole('button', { name: 'Make moderator' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm change' }));
    await vi.waitFor(() => expect(state.setChannelRole).toHaveBeenCalledWith('room', peer, 'moderator'));
  });
  it('explains removal limits and signs only the confirmed target', async () => {
    mount();
    expect(within(memberRow(founder)).queryByRole('button', { name: 'Remove member' })).not.toBeInTheDocument();
    fireEvent.click(within(memberRow(peer)).getByRole('button', { name: 'Remove member' }));
    expect(screen.getByRole('region', { name: 'Confirm encryption access change' })).toHaveTextContent('already received cannot be recalled');
    expect(state.removeChannelMember).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Confirm change' }));
    await vi.waitFor(() => expect(state.removeChannelMember).toHaveBeenCalledWith('room', peer));
  });
  it('requires explicit selection of the displayed certified device before admission', async () => {
    const certificate = { pseudonym_id: peer, device_id: device, server_identity: 'annex-server:' + '2'.repeat(64) };
    state.admissionDevices.mockResolvedValue([certificate]); mount();
    expect(screen.getByRole('button', { name: 'Load certified devices' })).toBeDisabled();
    fireEvent.change(screen.getByLabelText('Identity pseudonym'), { target: { value: peer } });
    fireEvent.click(screen.getByRole('button', { name: 'Load certified devices' }));
    await screen.findByRole('radio');
    expect(state.admissionDevices).toHaveBeenCalledWith('room', peer);
    expect(screen.getByRole('button', { name: 'Admit member' })).toBeDisabled();
    fireEvent.click(screen.getByRole('radio'));
    fireEvent.click(screen.getByRole('button', { name: 'Admit member' }));
    await vi.waitFor(() => expect(state.admitChannelMember).toHaveBeenCalledWith('room', certificate));
  });
  it('keeps a signed permission rejection visible and permits retry without refreshing it away', async () => {
    state.removeChannelMember.mockRejectedValueOnce(new Error('This action requires an authorized channel moderator')); mount();
    fireEvent.click(within(memberRow(peer)).getByRole('button', { name: 'Remove member' }));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm change' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('authorized channel moderator');
    expect(state.inspectChannel).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Confirm change' })).toBeEnabled();
  });
  it('blocks suspended-session mutations while preserving fingerprint inspection', () => {
    state.wsSessionState = 'taken_over'; mount();
    expect(screen.getByRole('dialog')).toHaveTextContent('e'.repeat(64));
    expect(screen.getByLabelText('Admission policy')).toBeDisabled();
    expect(within(memberRow(peer)).getByRole('button', { name: 'Remove member' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Done' })).toBeEnabled();
  });
});
