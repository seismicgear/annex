/**
 * Severing a federation agreement.
 *
 * `DELETE /api/admin/federation/{id}` had no caller and could not have had
 * one: it takes an agreement id, and nothing a client could see returned an
 * agreement id. `GET /api/public/federation/peers` sent base URL, label,
 * alignment and scope — no identifier at all. An operator who had stopped
 * trusting a peer had no way to cut it off from anywhere in the app.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const mockGetPeers = vi.fn();
const mockRevoke = vi.fn();

vi.mock('@/lib/api', () => ({
  getFederationPeers: (...a: unknown[]) => mockGetPeers(...a),
  revokeFederationAgreement: (...a: unknown[]) => mockRevoke(...a),
}));

function peer(over: Record<string, unknown> = {}) {
  return {
    agreement_id: 11,
    base_url: 'https://alpha.example',
    label: 'Alpha Station',
    alignment_status: 'Aligned',
    transfer_scope: 'FullKnowledgeBundle',
    active: true,
    ...over,
  };
}

async function renderAgreements() {
  vi.resetModules();
  const { FederationAgreements } = await import('./FederationAgreements');
  const user = userEvent.setup();
  render(<FederationAgreements pseudonymId="p1" />);
  await waitFor(() =>
    expect(
      document.querySelector('.agreement-list, .agreements-empty') ??
        screen.queryByRole('alert'),
    ).not.toBeNull(),
  );
  return user;
}

describe('FederationAgreements', () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(() => cleanup());

  it('lists each agreement with the peer it is with', async () => {
    mockGetPeers.mockResolvedValue({ peers: [peer()] });

    await renderAgreements();

    expect(screen.getByText('Alpha Station')).toBeInTheDocument();
    expect(screen.getByText('https://alpha.example')).toBeInTheDocument();
  });

  it('does not render a failed read as a server that federates with nobody', async () => {
    mockGetPeers.mockRejectedValue(new Error('gateway timeout'));

    await renderAgreements();

    expect(screen.getByRole('alert')).toHaveTextContent(
      /could not read the federation agreements.*gateway timeout/i,
    );
    expect(screen.queryByText(/no federation agreements/i)).not.toBeInTheDocument();
  });

  it('confirms before severing, and says what severing costs', async () => {
    mockGetPeers.mockResolvedValue({ peers: [peer()] });

    const user = await renderAgreements();
    await user.click(screen.getByRole('button', { name: 'Sever' }));

    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveTextContent(/sever federation with Alpha Station/i);
    expect(dialog).toHaveTextContent(/both directions/i);
    expect(dialog).toHaveTextContent(/cannot be undone from here/i);
    expect(mockRevoke).not.toHaveBeenCalled();
  });

  it('severs by agreement id, not by URL or position', async () => {
    // Two agreements can name one instance — the schema has no unique
    // constraint — so anything but the id can sever the wrong one.
    mockGetPeers.mockResolvedValue({
      peers: [
        peer({ agreement_id: 11, transfer_scope: 'FullKnowledgeBundle' }),
        peer({ agreement_id: 12, transfer_scope: 'ReflectionSummariesOnly' }),
      ],
    });

    const user = await renderAgreements();
    const rows = document.querySelectorAll('.agreement-row');
    expect(rows).toHaveLength(2);
    await user.click(
      within(rows[1] as HTMLElement).getByRole('button', { name: 'Sever' }),
    );
    await user.click(screen.getByRole('button', { name: 'Sever agreement' }));

    await waitFor(() => expect(mockRevoke).toHaveBeenCalledWith('p1', 12));
  });

  it('re-reads the list after severing rather than assuming the row is gone', async () => {
    mockGetPeers
      .mockResolvedValueOnce({ peers: [peer()] })
      .mockResolvedValueOnce({ peers: [] });
    mockRevoke.mockResolvedValue({ status: 'ok' });

    const user = await renderAgreements();
    await user.click(screen.getByRole('button', { name: 'Sever' }));
    await user.click(screen.getByRole('button', { name: 'Sever agreement' }));

    await waitFor(() => expect(mockGetPeers).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('status')).toHaveTextContent(/Alpha Station severed/i);
    expect(screen.getByText(/no federation agreements/i)).toBeInTheDocument();
  });

  it('keeps the dialog open and says why when the sever is refused', async () => {
    // Closing on failure would leave the peer listed with no explanation,
    // which reads as "it worked and the list is stale".
    mockGetPeers.mockResolvedValue({ peers: [peer()] });
    mockRevoke.mockRejectedValue(new Error('federation agreement not found or already revoked'));

    const user = await renderAgreements();
    await user.click(screen.getByRole('button', { name: 'Sever' }));
    await user.click(screen.getByRole('button', { name: 'Sever agreement' }));

    await waitFor(() =>
      expect(within(screen.getByRole('dialog')).getByRole('alert')).toHaveTextContent(
        /could not sever the agreement.*already revoked/i,
      ),
    );
    expect(screen.getByRole('dialog')).toBeInTheDocument();
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('says plainly when there are no agreements', async () => {
    mockGetPeers.mockResolvedValue({ peers: [] });

    await renderAgreements();

    expect(screen.getByText(/this server has no federation agreements/i)).toBeInTheDocument();
  });
});
