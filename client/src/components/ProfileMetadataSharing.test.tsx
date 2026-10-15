import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ProfileMetadataSharing } from './ProfileMetadataSharing';
const mocks = vi.hoisted(() => ({
  list: vi.fn(), directory: vi.fn(), change: vi.fn(),
  state: { session: 'active', serial: 1, pseudonymId: 'owner' },
}));
vi.mock('@/api/profile-metadata', () => ({ listProfileMetadataGrants: mocks.list, setProfileMetadataGrant: mocks.change }));
vi.mock('@/api/members', () => ({ listDirectoryMembers: mocks.directory }));
vi.mock('@/api/core', () => ({
  getCredentialContext: () => ({ serial: mocks.state.serial, pseudonymId: mocks.state.pseudonymId }),
  isCredentialContextCurrent: (context: { serial: number }) => context.serial === mocks.state.serial,
}));
vi.mock('@/stores/channels', () => ({ useChannelsStore: (select: (value: unknown) => unknown) => select({ wsSessionState: mocks.state.session }) }));
const initial = { owner_pseudonym: 'owner', metadata_json: null, grantees: [] as string[] };
beforeEach(() => {
  vi.clearAllMocks(); mocks.state.session = 'active'; mocks.state.serial = 1; mocks.state.pseudonymId = 'owner';
  mocks.list.mockResolvedValue(initial); mocks.change.mockResolvedValue(undefined);
  mocks.directory.mockResolvedValue([{ pseudonym_id: 'owner', participant_type: 'HUMAN' }, { pseudonym_id: 'reader', participant_type: 'HUMAN' }]);
});
afterEach(cleanup);
async function open() {
  const view = render(<ProfileMetadataSharing pseudonymId="owner" />);
  expect(mocks.list).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Manage profile sharing' }));
  await screen.findByRole('button', { name: 'Grant profile sharing' });
  return view;
}
describe('explicit profile details sharing', () => {
  it('honestly describes an empty human profile, previews future scope and cancels without writing', async () => {
    await open();
    expect(screen.getByText('No additional profile details are stored for this identity.')).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Grant profile sharing' }));
    const confirmation = screen.getByRole('region', { name: 'Confirm profile sharing change' });
    expect(within(confirmation).getByText(/future updates/)).toBeTruthy();
    fireEvent.click(within(confirmation).getByRole('button', { name: 'Cancel sharing change' }));
    expect(mocks.change).not.toHaveBeenCalled();
    expect(screen.queryByRole('region', { name: 'Confirm profile sharing change' })).toBeNull();
  });

  it('previews actual JSON, grants only after confirmation and requires a verified readback', async () => {
    const value = { ...initial, metadata_json: '{"alignment_status":"Aligned","capability_contract":{"offered_capabilities":["text"]}}' };
    mocks.list.mockResolvedValue(value);
    await open();
    expect(screen.getByLabelText('Current profile details').textContent).toBe(value.metadata_json);
    fireEvent.click(screen.getByRole('button', { name: 'Grant profile sharing' }));
    expect(mocks.change).not.toHaveBeenCalled();
    mocks.list.mockResolvedValue({ ...value, grantees: ['reader'] });
    fireEvent.click(screen.getByRole('button', { name: 'Confirm sharing change' }));
    await screen.findByText('Profile details sharing granted.');
    expect(mocks.change).toHaveBeenCalledWith('reader', true);
    expect(screen.getByRole('button', { name: 'Revoke profile sharing' })).toBeTruthy();
  });

  it('keeps an inactive grantee revocable even when directory loading fails', async () => {
    mocks.list.mockResolvedValue({ ...initial, grantees: ['inactive-reader'] });
    mocks.directory.mockRejectedValue(new Error('Directory unavailable'));
    render(<ProfileMetadataSharing pseudonymId="owner" />);
    fireEvent.click(screen.getByRole('button', { name: 'Manage profile sharing' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Revoke profile sharing' }));
    expect(screen.getByText(/Not in the active member list/)).toBeTruthy();
    mocks.list.mockResolvedValue(initial);
    fireEvent.click(screen.getByRole('button', { name: 'Confirm sharing change' }));
    await screen.findByText('Profile details sharing revoked.');
    expect(mocks.change).toHaveBeenCalledWith('inactive-reader', false);
  });

  it('does not claim a lost acknowledgement left the old privacy state intact', async () => {
    await open();
    fireEvent.click(screen.getByRole('button', { name: 'Grant profile sharing' }));
    mocks.change.mockRejectedValue(new Error('Connection interrupted'));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm sharing change' }));
    await screen.findByText(/change could not be confirmed/);
    expect(screen.queryByRole('button', { name: 'Grant profile sharing' })).toBeNull();
    expect(screen.queryByText('Profile details sharing granted.')).toBeNull();
    mocks.list.mockResolvedValue({ ...initial, grantees: ['reader'] });
    fireEvent.click(screen.getByRole('button', { name: 'Retry profile sharing' }));
    await screen.findByRole('button', { name: 'Revoke profile sharing' });
  });

  it('does not announce success when an acknowledged grant cannot be read back', async () => {
    await open();
    fireEvent.click(screen.getByRole('button', { name: 'Grant profile sharing' }));
    mocks.list.mockRejectedValue(new Error('Read failed'));
    fireEvent.click(screen.getByRole('button', { name: 'Confirm sharing change' }));
    await screen.findByText(/acknowledged, but the current sharing state could not be confirmed/);
    expect(screen.queryByText('Profile details sharing granted.')).toBeNull();
  });

  it('blocks a pending confirmation when the live identity is taken over', async () => {
    const view = await open();
    fireEvent.click(screen.getByRole('button', { name: 'Grant profile sharing' }));
    mocks.state.session = 'taken_over';
    view.rerender(<ProfileMetadataSharing pseudonymId="owner" />);
    const button = screen.getByRole('button', { name: 'Confirm sharing change' }) as HTMLButtonElement;
    expect(button.disabled).toBe(true);
    fireEvent.click(button); expect(mocks.change).not.toHaveBeenCalled();
  });

  it('drops a delayed snapshot from a previous credential context', async () => {
    let finish!: (value: typeof initial) => void;
    mocks.list.mockReturnValue(new Promise(resolve => { finish = resolve; }));
    render(<ProfileMetadataSharing pseudonymId="owner" />);
    fireEvent.click(screen.getByRole('button', { name: 'Manage profile sharing' }));
    mocks.state.serial++;
    await act(async () => finish(initial));
    expect(screen.queryByText('No additional profile details are stored for this identity.')).toBeNull();
    expect(screen.queryByRole('button', { name: 'Grant profile sharing' })).toBeNull();
  });
});
