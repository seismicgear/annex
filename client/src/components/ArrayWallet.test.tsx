import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { ArrayWalletControls } from './ArrayWalletControls';
import { ArrayWalletRestore } from './ArrayWalletRestore';

const mocks = vi.hoisted(() => ({ status: vi.fn(), disconnect: vi.fn(), list: vi.fn(), get: vi.fn(), importBackup: vi.fn(), compute: vi.fn() }));
vi.mock('@/lib/tauri', () => ({
  isTauri: () => true, getArrayWalletStatus: mocks.status, disconnectArrayWallet: mocks.disconnect,
  listArrayWalletBackups: mocks.list, getArrayIdentityBackup: mocks.get,
}));
vi.mock('@/stores/identity', () => ({ useIdentityStore: { getState: () => ({ importBackup: mocks.importBackup }) } }));
vi.mock('@/lib/zk', () => ({ computeCommitment: mocks.compute }));

const managed = { managedBy: 'array', paired: true, available: true, accountLabel: 'Test account', message: null };
const standalone = { managedBy: null, paired: false, available: false, accountLabel: null, message: null };
const recovery = { sk: '1', roleCode: 1, nodeId: 0, commitmentHex: 'a'.repeat(64), pseudonymId: 'member', serverSlug: 'alpha', createdAt: '2026-10-19T00:00:00Z' };

beforeEach(() => {
  vi.clearAllMocks();
  mocks.status.mockResolvedValue(managed);
  mocks.list.mockResolvedValue([{ id: 'backup-11111111', label: 'Annex ZKP key' }, { id: 'backup-22222222', label: 'Annex ZKP key' }]);
  mocks.get.mockResolvedValue({ format: 'annex.identity.v1', value: { ...recovery, sessionToken: 'old-session', zkProofPayload: 'old-proof' }, managedBy: 'array' });
  mocks.compute.mockResolvedValue('a'.repeat(64));
  mocks.importBackup.mockResolvedValue(undefined);
  mocks.disconnect.mockImplementation(async () => { mocks.status.mockResolvedValue(standalone); return standalone; });
});

describe('Array pairing controls', () => {
  it('shows ownership and disconnects through Array before switching to standalone', async () => {
    render(<ArrayWalletControls />);
    await screen.findByText('Managed by Array');
    fireEvent.click(screen.getByRole('button', { name: 'Disconnect Array pairing' }));
    await screen.findByText(/Close Annex, then open it from Array/);
    expect(mocks.disconnect).toHaveBeenCalledTimes(1);
    expect(screen.queryByText('Managed by Array')).not.toBeInTheDocument();
  });

  it('keeps managed status when revocation is not acknowledged and hides private diagnostics', async () => {
    mocks.disconnect.mockRejectedValue('private native diagnostic');
    render(<ArrayWalletControls />);
    await screen.findByText('Managed by Array');
    fireEvent.click(screen.getByRole('button', { name: 'Disconnect Array pairing' }));
    await screen.findByText('Could not disconnect Array. The pairing remains active; try again.');
    expect(screen.getByText('Managed by Array')).toBeVisible();
    expect(screen.queryByText('private native diagnostic')).not.toBeInTheDocument();
  });
});

describe('Array identity restoration', () => {
  async function chooseBackup() {
    await screen.findByRole('button', { name: 'Choose an Array backup' });
    fireEvent.click(screen.getByRole('button', { name: 'Choose an Array backup' }));
    await screen.findByRole('combobox', { name: 'Annex identity backup' });
  }

  it('distinguishes multiple backups, verifies key binding, and reuses Annex import without source sessions', async () => {
    render(<ArrayWalletRestore />);
    await chooseBackup();
    expect(screen.getByRole('option', { name: 'Annex ZKP key · 11111111' })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: 'Annex ZKP key · 22222222' })).toBeInTheDocument();
    fireEvent.change(screen.getByRole('combobox'), { target: { value: 'backup-22222222' } });
    fireEvent.click(screen.getByRole('button', { name: 'Restore selected identity' }));
    await waitFor(() => expect(mocks.importBackup).toHaveBeenCalledTimes(1));
    expect(mocks.get).toHaveBeenCalledWith('backup-22222222');
    expect(mocks.compute).toHaveBeenCalledWith(1n, 1, 0);
    expect(JSON.parse(mocks.importBackup.mock.calls[0][0])).toEqual(recovery);
  });

  it('rejects mismatched cryptographic identity before touching existing records', async () => {
    mocks.compute.mockResolvedValue('b'.repeat(64));
    render(<ArrayWalletRestore />);
    await chooseBackup();
    fireEvent.click(screen.getByRole('button', { name: 'Restore selected identity' }));
    await screen.findByText('The Annex backup does not match its cryptographic identity.');
    expect(mocks.importBackup).not.toHaveBeenCalled();
  });

  it('ignores a late secret response after the restore screen has closed', async () => {
    let complete!: (value: unknown) => void;
    mocks.get.mockReturnValue(new Promise(resolve => { complete = resolve; }));
    const view = render(<ArrayWalletRestore />);
    await chooseBackup();
    fireEvent.click(screen.getByRole('button', { name: 'Restore selected identity' }));
    await waitFor(() => expect(mocks.get).toHaveBeenCalledTimes(1));
    view.unmount();
    await act(async () => { complete({ format: 'annex.identity.v1', value: recovery, managedBy: 'array' }); });
    expect(mocks.importBackup).not.toHaveBeenCalled();
  });

  it('does not import a pending recovery after the pairing is revoked', async () => {
    let complete!: (value: unknown) => void;
    mocks.get.mockReturnValue(new Promise(resolve => { complete = resolve; }));
    render(<ArrayWalletRestore />);
    await chooseBackup();
    fireEvent.click(screen.getByRole('button', { name: 'Restore selected identity' }));
    await waitFor(() => expect(mocks.get).toHaveBeenCalledTimes(1));
    mocks.status.mockResolvedValue({ ...managed, paired: false, message: 'Pair Annex in Array Settings to use its credential wallet.' });
    fireEvent(window, new Event('focus'));
    await screen.findByText('Pair Annex in Array Settings to use its credential wallet.');
    await act(async () => { complete({ format: 'annex.identity.v1', value: recovery, managedBy: 'array' }); });
    expect(mocks.importBackup).not.toHaveBeenCalled();
  });
});
