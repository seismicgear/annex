/**
 * The edit-history panel told the user something the server never said.
 *
 * `handleShowHistory` wrapped `api.getMessageEdits` in a bare
 * `catch { setEditHistory([]) }`, and an empty array renders as **"No edit
 * history found"**. So a 403, a 500 or a dropped connection was reported as
 * a fact about the message: *this was never edited*. It appeared on a
 * message visibly carrying an "(edited)" badge, which is the one place the
 * claim is provably false — and the audit trail is exactly the thing a user
 * opens this panel to trust.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const mockGetMessageEdits = vi.fn();

vi.mock('@/lib/api', () => ({
  getMessageEdits: (...a: unknown[]) => mockGetMessageEdits(...a),
  resolveUrl: (u: string) => u,
  getVisibleUsernames: vi.fn(async () => ({ usernames: {} })),
}));

vi.mock('@/lib/personas', () => ({
  getPersonasForIdentity: vi.fn(async () => []),
}));

vi.mock('@/components/LinkPreview', () => ({
  LinkPreview: () => null,
}));

const EDITED_MESSAGE = {
  message_id: 'm-1',
  channel_id: 'ch-1',
  sender_pseudonym: 'p-self',
  content: 'the current text',
  created_at: new Date().toISOString(),
  edited_at: new Date().toISOString(),
  deleted_at: null,
};

async function renderWithEditedMessage() {
  // A fresh module registry per test: the stores are module singletons, so
  // without this one test's channel/identity state bleeds into the next.
  vi.resetModules();
  const { useIdentityStore } = await import('@/stores/identity');
  const { useChannelsStore } = await import('@/stores/channels');
  const { MessageView } = await import('./MessageView');

  useIdentityStore.setState({
    identity: {
      id: 'i1', sk: 'x', pseudonymId: 'p-self', sessionToken: 't', commitmentHex: 'c',
      roleCode: 0, nodeId: 'n', serverSlug: 's', leafIndex: 0, createdAt: '',
    } as never,
  });
  useChannelsStore.setState({
    activeChannelId: 'ch-1',
    messages: [EDITED_MESSAGE] as never,
  });

  render(<MessageView />);
  return userEvent.setup();
}

describe('edit history — a failed fetch is not an empty history', () => {
  beforeEach(() => {
    // `mockReset`, not `clearAllMocks`: a `…Once` queued by a test that
    // failed before consuming it would otherwise fire in the next one.
    mockGetMessageEdits.mockReset();
    // jsdom does not implement it; the component auto-scrolls on mount.
    Element.prototype.scrollIntoView = vi.fn();
  });
  afterEach(() => {
    cleanup();
  });

  it('does not claim "No edit history found" when the request fails', async () => {
    mockGetMessageEdits.mockRejectedValue(new Error('network down'));
    const user = await renderWithEditedMessage();

    await user.click(screen.getByTitle('Show edit history'));

    await waitFor(() => {
      expect(screen.queryByText(/No edit history found/i)).not.toBeInTheDocument();
    });
    expect(screen.getByText(/Could not load edit history/i)).toBeInTheDocument();
  });

  it('offers a retry that succeeds on the second attempt', async () => {
    mockGetMessageEdits
      .mockRejectedValueOnce(new Error('network down'))
      .mockResolvedValueOnce([
        { id: 1, old_content: 'the older text', edited_at: new Date().toISOString() },
      ]);
    const user = await renderWithEditedMessage();

    await user.click(screen.getByTitle('Show edit history'));
    await screen.findByText(/Could not load edit history/i);

    await user.click(screen.getByRole('button', { name: /retry/i }));

    expect(await screen.findByText('the older text')).toBeInTheDocument();
    expect(screen.queryByText(/Could not load edit history/i)).not.toBeInTheDocument();
  });

  it('still says the history is empty when the server really returns none', async () => {
    mockGetMessageEdits.mockResolvedValue([]);
    const user = await renderWithEditedMessage();

    await user.click(screen.getByTitle('Show edit history'));

    expect(await screen.findByText(/No edit history found/i)).toBeInTheDocument();
  });
});
