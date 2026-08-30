/**
 * The federation delivery queue.
 *
 * `GET /api/admin/federation/outbox` and its per-row retry shipped with no
 * caller in the client. The list handler's doc comment says the status counts
 * exist so "the UI show[s] queue depth and stuck deliveries at a glance" —
 * for a UI that did not exist. A server whose every federation delivery was
 * failing looked, from every screen in the app, identical to one with nothing
 * to send.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const mockGetOutbox = vi.fn();
const mockRetry = vi.fn();

vi.mock('@/lib/api', () => ({
  getFederationOutbox: (...a: unknown[]) => mockGetOutbox(...a),
  retryFederationOutboxRow: (...a: unknown[]) => mockRetry(...a),
}));

function row(over: Record<string, unknown> = {}) {
  return {
    id: 1,
    peer_instance_id: 7,
    peer_base_url: 'https://peer.example',
    peer_label: 'Peer One',
    message_id: 'msg-abc',
    status: 'failed',
    attempts: 5,
    next_retry_at: '2026-03-01 10:00:00',
    last_error: 'connection refused',
    created_at: '2026-03-01 09:00:00',
    updated_at: '2026-03-01 09:55:00',
    envelope_bytes: 2048,
    ...over,
  };
}

function page(entries: unknown[], counts: Record<string, number> = {}) {
  return { entries, counts, limit: 50, offset: 0 };
}

async function renderOutbox() {
  vi.resetModules();
  const { FederationOutbox } = await import('./FederationOutbox');
  const user = userEvent.setup();
  render(<FederationOutbox pseudonymId="p1" />);
  // The initial fetch resolves after render either way; wait for whichever
  // branch it produced so no state update lands outside act().
  await waitFor(() =>
    expect(
      document.querySelector('.outbox-counts') ?? screen.queryByRole('alert'),
    ).not.toBeNull(),
  );
  return user;
}

describe('FederationOutbox', () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(() => cleanup());

  it('shows queue depth per status and the failing row with its reason', async () => {
    mockGetOutbox.mockResolvedValue(
      page([row()], { pending: 3, failed: 1, delivered: 42 }),
    );

    await renderOutbox();

    const failed = await screen.findByText('failed', { selector: '.outbox-count-label' });
    expect(failed.parentElement).toHaveTextContent('1');
    expect(screen.getByText('Peer One')).toBeInTheDocument();
    expect(screen.getByText('connection refused')).toBeInTheDocument();
    expect(screen.getByText('5 attempts')).toBeInTheDocument();
    expect(screen.getByText('2.0 KB')).toBeInTheDocument();
  });

  it('does not render a failed read as an empty queue', async () => {
    // The one thing this panel must never do. "Nothing queued" and "we could
    // not ask" are the same picture unless the code keeps them apart, and the
    // first tells the operator every envelope has been delivered.
    mockGetOutbox.mockRejectedValue(new Error('insufficient permissions'));

    await renderOutbox();

    expect(await screen.findByRole('alert')).toHaveTextContent(
      /could not read the delivery queue.*insufficient permissions/i,
    );
    expect(screen.queryByText(/nothing has been queued/i)).not.toBeInTheDocument();
  });

  it('distinguishes an empty queue from an empty filter', async () => {
    // "No failed deliveries" is good news; "nothing has ever been queued" is
    // a different fact, and an operator checking whether federation works at
    // all needs to be able to tell which one they are looking at.
    mockGetOutbox.mockResolvedValue(page([], { pending: 0, delivered: 12 }));

    const user = await renderOutbox();
    await screen.findByRole('combobox');
    await user.selectOptions(screen.getByRole('combobox'), 'failed');

    await waitFor(() =>
      expect(screen.getByText(/no failed deliveries/i)).toHaveTextContent(
        /queue holds 12 in other states/i,
      ),
    );
  });

  it('says nothing has been queued when nothing has', async () => {
    mockGetOutbox.mockResolvedValue(page([], {}));

    await renderOutbox();

    expect(await screen.findByText(/nothing has been queued for a federation peer yet/i))
      .toBeInTheDocument();
  });

  it('offers Retry only on the two statuses the server accepts', async () => {
    // `pending` and `delivered` are answered with a 409 — a button that can
    // only produce an error is a worse affordance than no button.
    mockGetOutbox.mockResolvedValue(
      page([
        row({ id: 1, status: 'failed', message_id: 'm-failed' }),
        row({ id: 2, status: 'paused', message_id: 'm-paused' }),
        row({ id: 3, status: 'pending', message_id: 'm-pending', last_error: null }),
        row({ id: 4, status: 'delivered', message_id: 'm-delivered', last_error: null }),
      ]),
    );

    await renderOutbox();

    await screen.findByText('m-failed', { exact: false });
    const rows = document.querySelectorAll('.outbox-row');
    expect(rows).toHaveLength(4);
    const hasRetry = [...rows].map(
      (r) => within(r as HTMLElement).queryByRole('button', { name: 'Retry' }) !== null,
    );
    expect(hasRetry).toEqual([true, true, false, false]);
  });

  it('re-reads the queue after a retry rather than assuming the row moved', async () => {
    mockGetOutbox
      .mockResolvedValueOnce(page([row()], { failed: 1 }))
      .mockResolvedValueOnce(page([row({ status: 'pending', last_error: null })], { pending: 1 }));
    mockRetry.mockResolvedValue({
      status: 'ok',
      outbox_id: 1,
      message_id: 'msg-abc',
      new_status: 'pending',
    });

    const user = await renderOutbox();
    await user.click(await screen.findByRole('button', { name: 'Retry' }));

    await waitFor(() => expect(mockGetOutbox).toHaveBeenCalledTimes(2));
    expect(screen.getByRole('status')).toHaveTextContent(/msg-abc is back in the retry rotation/i);
    expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
  });

  it('reports a refused retry instead of implying it worked', async () => {
    mockGetOutbox.mockResolvedValue(page([row()], { failed: 1 }));
    mockRetry.mockRejectedValue(
      new Error("federation outbox row 1 is 'delivered' — only 'failed' or 'paused' rows can be retried"),
    );

    const user = await renderOutbox();
    await user.click(await screen.findByRole('button', { name: 'Retry' }));

    await waitFor(() =>
      expect(screen.getByText(/could not retry that delivery/i)).toHaveTextContent(
        /only 'failed' or 'paused' rows can be retried/,
      ),
    );
    expect(screen.queryByRole('status')).not.toBeInTheDocument();
  });

  it('names a peer whose instance row is gone by its id, not "undefined"', async () => {
    // The outbox outlives the instances table: a row can be queued for a peer
    // that has since been removed, and the join then returns nulls.
    mockGetOutbox.mockResolvedValue(
      page([row({ peer_label: null, peer_base_url: null, peer_instance_id: 7 })]),
    );

    await renderOutbox();

    expect(await screen.findByText('peer #7 (removed)')).toBeInTheDocument();
  });

  it('asks the server for the selected status rather than filtering locally', async () => {
    // The page is capped at 50 rows, so a client-side filter would hide
    // failures that exist but did not fit on the first page.
    mockGetOutbox.mockResolvedValue(page([], { pending: 1 }));

    const user = await renderOutbox();
    await screen.findByRole('combobox');
    await user.selectOptions(screen.getByRole('combobox'), 'paused');

    await waitFor(() =>
      expect(mockGetOutbox).toHaveBeenLastCalledWith('p1', { status: 'paused', limit: 50 }),
    );
  });
});

describe('FederationOutbox failure tally', () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(() => cleanup());

  it('tints the failed count only when something has actually failed', async () => {
    // A red zero is an alarm for a condition that is not happening. An
    // operator who sees one on every healthy server learns to stop reading
    // the panel, which costs more than the colour buys.
    mockGetOutbox.mockResolvedValue(page([], { pending: 0, failed: 0, delivered: 9 }));

    await renderOutbox();

    const failed = document.querySelector('.outbox-count-failed');
    expect(failed).not.toBeNull();
    expect(failed).not.toHaveClass('outbox-count-nonzero');
  });

  it('tints it when something has', async () => {
    mockGetOutbox.mockResolvedValue(page([row()], { failed: 2 }));

    await renderOutbox();

    expect(document.querySelector('.outbox-count-failed')).toHaveClass('outbox-count-nonzero');
  });
});
