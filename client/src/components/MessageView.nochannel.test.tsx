/**
 * What the message pane says when no channel is selected.
 *
 * "Select a channel to start chatting" is right when channels exist and the
 * user has not picked one. It is an instruction the user cannot follow when
 * the sidebar is empty because its request FAILED — the rate-limited and
 * offline states both land here, and the page then said two different things
 * at once: the channel list reported an error, and the pane beside it implied
 * nothing was wrong and invited an action with nothing to act on.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';

vi.mock('@/lib/api', () => ({
  getMessageEdits: vi.fn(),
  resolveUrl: (u: string) => u,
  getVisibleUsernames: vi.fn(async () => ({ usernames: {} })),
}));

vi.mock('@/lib/personas', () => ({
  getPersonasForIdentity: vi.fn(async () => []),
}));

vi.mock('@/components/LinkPreview', () => ({
  LinkPreview: () => null,
}));

async function renderWithChannelState(state: Record<string, unknown>) {
  vi.resetModules();
  const { useChannelsStore } = await import('@/stores/channels');
  const { MessageView } = await import('./MessageView');
  useChannelsStore.setState({ activeChannelId: null, messages: [], ...state } as never);
  render(<MessageView />);
}

describe('message pane with no channel selected', () => {
  beforeEach(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });
  afterEach(() => cleanup());

  it('invites the user to pick one when the list loaded fine', async () => {
    await renderWithChannelState({ error: null, loading: false });
    expect(screen.getByText('Select a channel to start chatting')).toBeInTheDocument();
  });

  it('does not invite a choice from a list that failed to load', async () => {
    await renderWithChannelState({ error: 'Rate limit exceeded. Try again in 60 seconds.', loading: false });
    expect(screen.queryByText('Select a channel to start chatting')).not.toBeInTheDocument();
    expect(screen.getByText(/channels could not be loaded/i)).toBeInTheDocument();
  });

  it('says it is still loading rather than inviting a choice too early', async () => {
    await renderWithChannelState({ error: null, loading: true });
    expect(screen.queryByText('Select a channel to start chatting')).not.toBeInTheDocument();
    expect(screen.getByText('Loading channels...')).toBeInTheDocument();
  });

  it('keeps the live region in every one of those branches', async () => {
    // The empty branches share one shell precisely so `role="log"` and the
    // focusable scroll container survive; a branch that dropped them is the
    // defect class this file exists to guard.
    for (const state of [
      { error: null, loading: false },
      { error: 'boom', loading: false },
      { error: null, loading: true },
    ]) {
      await renderWithChannelState(state);
      expect(screen.getByRole('log', { name: 'Message history' })).toBeInTheDocument();
      cleanup();
    }
  });
});
