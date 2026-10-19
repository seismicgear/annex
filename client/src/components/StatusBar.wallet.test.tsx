import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { StatusBar } from './StatusBar';

const state = vi.hoisted(() => ({
  identity: { id: 'local-id', sk: '1', roleCode: 1, nodeId: 1, commitmentHex: 'a'.repeat(64), pseudonymId: 'member', serverSlug: 'alpha', createdAt: '2026-10-19T00:00:00Z', sessionToken: 'source-session', zkProofPayload: 'source-proof' },
  status: vi.fn(), save: vi.fn(), file: vi.fn(),
}));
vi.mock('@/lib/tauri', () => ({
  isTauri: () => true, getArrayWalletStatus: state.status, saveArrayIdentityBackup: state.save, exportIdentityJson: state.file,
}));
vi.mock('@/stores/identity', () => ({
  useIdentityStore: (select: (value: unknown) => unknown) => select({ identity: state.identity, logout: vi.fn(), exportCurrent: () => JSON.stringify(state.identity) }),
}));
vi.mock('@/stores/channels', () => ({
  useChannelsStore: (select: (value: unknown) => unknown) => select({ channels: [], wsConnected: true, wsSessionState: 'connected' }),
}));
vi.mock('@/stores/voice', () => ({ useVoiceStore: () => ({ voiceToken: null, connectionState: 'idle' }) }));
vi.mock('@/lib/personas', () => ({ getPersonasForIdentity: async () => [] }));
vi.mock('@/components/DeviceLinkDialog', () => ({ DeviceLinkDialog: () => null }));
vi.mock('@/components/IdentitySettings', () => ({ IdentitySettings: () => null }));
vi.mock('@/components/SocialRecoveryDialog', () => ({ SocialRecoveryDialog: () => null }));
vi.mock('@/components/AudioSettings', () => ({ AudioSettings: () => null }));
vi.mock('@/components/DesktopUpdates', () => ({ DesktopUpdates: () => null }));

const managed = { managedBy: 'array', available: true, paired: true, accountLabel: 'Test account', message: null };
describe('Annex backup destination', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    state.status.mockResolvedValue(managed);
    state.save.mockResolvedValue({ managedBy: 'array', credentialId: 'saved', savedAt: '2026-10-19T00:00:00Z' });
    state.file.mockResolvedValue('/tmp/annex-backup.json');
  });

  it('automatically saves a portable recovery bundle to Array and waits for its receipt', async () => {
    let complete!: (value: unknown) => void;
    state.save.mockReturnValue(new Promise(resolve => { complete = resolve; }));
    render(<StatusBar />);
    await screen.findByText('Managed by Array');
    fireEvent.click(screen.getByRole('button', { name: 'Export' }));
    await waitFor(() => expect(state.save).toHaveBeenCalledTimes(1));
    expect(state.save.mock.calls[0][0]).toEqual({ sk: '1', roleCode: 1, nodeId: 1, commitmentHex: 'a'.repeat(64), pseudonymId: 'member', serverSlug: 'alpha', createdAt: '2026-10-19T00:00:00Z' });
    expect(screen.queryByText('Identity backup saved in Array.')).not.toBeInTheDocument();
    complete({ managedBy: 'array', credentialId: 'saved', savedAt: '2026-10-19T00:00:00Z' });
    await screen.findByText('Identity backup saved in Array.');
    expect(state.file).not.toHaveBeenCalled();
  });

  it('keeps a managed failure visible and only exports a file after explicit selection', async () => {
    state.save.mockRejectedValue('Unlock the credential wallet in Array and try again.');
    render(<StatusBar />);
    await screen.findByText('Managed by Array');
    fireEvent.click(screen.getByRole('button', { name: 'Export' }));
    await screen.findByText('Unlock the credential wallet in Array and try again.');
    expect(state.file).not.toHaveBeenCalled();
    expect(screen.getByRole('button', { name: 'Retry Array backup' })).toBeEnabled();
    fireEvent.click(screen.getByRole('button', { name: 'Export backup file instead' }));
    await waitFor(() => expect(state.file).toHaveBeenCalledTimes(1));
    await screen.findByText('Identity exported to /tmp/annex-backup.json');
  });

  it('does not silently switch a dead managed connection to file storage', async () => {
    state.status.mockResolvedValue({ ...managed, available: false, paired: false, message: 'Array is unavailable. Reopen Annex from Array, or export a backup file.' });
    render(<StatusBar />);
    await screen.findByText('Managed by Array');
    fireEvent.click(screen.getByRole('button', { name: 'Export' }));
    await screen.findByRole('button', { name: 'Export backup file instead' });
    expect(state.save).not.toHaveBeenCalled();
    expect(state.file).not.toHaveBeenCalled();
  });

  it('retains ordinary file export for a standalone launch', async () => {
    state.status.mockResolvedValue({ managedBy: null, paired: false, available: false, accountLabel: null, message: null });
    render(<StatusBar />);
    fireEvent.click(screen.getByRole('button', { name: 'Export' }));
    await waitFor(() => expect(state.file).toHaveBeenCalledTimes(1));
    expect(state.save).not.toHaveBeenCalled();
  });

  it('does not report a malformed save response as a durable backup', async () => {
    state.save.mockResolvedValue({ managedBy: 'array' });
    render(<StatusBar />);
    fireEvent.click(screen.getByRole('button', { name: 'Export' }));
    await screen.findByRole('button', { name: 'Retry Array backup' });
    expect(screen.queryByText('Identity backup saved in Array.')).not.toBeInTheDocument();
  });
});
