/**
 * The storage gate in the admin panel.
 *
 * `GET /api/admin/storage` and `POST /api/admin/storage/clear` shipped with
 * no caller in the client. A server that ran out of disk therefore answered
 * every write with a 507 — the `storage-gate-507` audit surface is a picture
 * of that — while the admin panel, the one screen an operator opens when the
 * server stops accepting writes, said nothing about it and offered no way
 * out. The gate has no automatic recovery by design, so from the UI the only
 * recovery was still a process restart.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const mockGetStorageHealth = vi.fn();
const mockClearStorageGate = vi.fn();

vi.mock('@/lib/api', () => ({
  getStorageHealth: (...a: unknown[]) => mockGetStorageHealth(...a),
  clearStorageGate: (...a: unknown[]) => mockClearStorageGate(...a),
  getServer: vi.fn(async () => ({ slug: 'alpha', label: 'Alpha', public_url: '' })),
  getApiBaseUrl: () => 'https://alpha.example',
  resolveUrl: (u: string) => u,
  renameServer: vi.fn(),
  setPublicUrl: vi.fn(),
  uploadServerImage: vi.fn(),
}));

vi.mock('@/lib/invite', () => ({
  canCreateInviteLink: () => false,
  createInviteLink: vi.fn(),
}));

async function renderServerAdmin() {
  vi.resetModules();
  const { useIdentityStore } = await import('@/stores/identity');
  const { AdminPanel } = await import('./AdminPanel');

  useIdentityStore.setState({ identity: { pseudonymId: 'p1' } as never });

  const user = userEvent.setup();
  render(<AdminPanel section="server" />);
  // `ServerSettings` renders a loading placeholder until `getServer` lands,
  // so the storage section is not even mounted before this settles.
  await screen.findByDisplayValue('Alpha');
  return user;
}

describe('AdminPanel storage gate', () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(() => cleanup());

  it('reports a degraded gate and says writes are being rejected', async () => {
    mockGetStorageHealth.mockResolvedValue({
      state: 'degraded',
      reason: 'free space 41 MB below the 128 MB block threshold',
      writes_blocked: true,
    });

    await renderServerAdmin();

    expect(await screen.findByText('degraded')).toBeInTheDocument();
    expect(screen.getByText(/writes are being rejected/i)).toBeInTheDocument();
    expect(
      screen.getByText(/free space 41 MB below the 128 MB block threshold/),
    ).toBeInTheDocument();
  });

  it('does not render a failed read as a healthy server', async () => {
    // The one thing this panel must never do. A green light produced by a
    // dropped request is worse than no panel: the operator stops looking.
    mockGetStorageHealth.mockRejectedValue(new Error('permission denied'));

    await renderServerAdmin();

    expect(await screen.findByRole('alert')).toHaveTextContent(
      /could not read storage health.*permission denied/i,
    );
    expect(screen.queryByText('healthy')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /clear storage gate/i })).not.toBeInTheDocument();
  });

  it('offers no clear button while the gate is healthy', async () => {
    mockGetStorageHealth.mockResolvedValue({ state: 'healthy', reason: '', writes_blocked: false });

    await renderServerAdmin();

    expect(await screen.findByText('healthy')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /clear storage gate/i })).not.toBeInTheDocument();
  });

  it('distinguishes warn from degraded — warn still accepts writes', async () => {
    // `warn` is the state that exists to be seen before anything breaks. If
    // it read the same as `degraded`, an operator would either panic at it or
    // learn to ignore both.
    mockGetStorageHealth.mockResolvedValue({
      state: 'warn',
      reason: 'free space 900 MB below the 1 GB warn threshold',
      writes_blocked: false,
    });

    await renderServerAdmin();

    expect(await screen.findByText('warn')).toBeInTheDocument();
    expect(screen.getByText(/writes are still being accepted/i)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /clear storage gate/i })).toBeInTheDocument();
  });

  it('clears the gate and re-reads the state rather than assuming it worked', async () => {
    mockGetStorageHealth
      .mockResolvedValueOnce({ state: 'degraded', reason: 'disk full', writes_blocked: true })
      .mockResolvedValueOnce({ state: 'healthy', reason: '', writes_blocked: false });
    mockClearStorageGate.mockResolvedValue({
      status: 'ok',
      previous_state: 'degraded',
      state: 'healthy',
    });

    const user = await renderServerAdmin();
    await user.click(await screen.findByRole('button', { name: /clear storage gate/i }));

    await waitFor(() => expect(screen.getByText('healthy')).toBeInTheDocument());
    expect(mockGetStorageHealth).toHaveBeenCalledTimes(2);
    expect(screen.getByRole('status')).toHaveTextContent(/writes are being accepted again/i);
  });

  it('says so when the clear itself fails, and leaves the gate showing tripped', async () => {
    mockGetStorageHealth.mockResolvedValue({
      state: 'degraded',
      reason: 'disk full',
      writes_blocked: true,
    });
    mockClearStorageGate.mockRejectedValue(new Error('insufficient permissions'));

    const user = await renderServerAdmin();
    await user.click(await screen.findByRole('button', { name: /clear storage gate/i }));

    await waitFor(() =>
      expect(screen.getByText(/could not clear the storage gate/i)).toHaveTextContent(
        /insufficient permissions/,
      ),
    );
    expect(screen.getByText('degraded')).toBeInTheDocument();
  });
});
