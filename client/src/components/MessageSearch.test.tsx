/**
 * The search panel's three empty states, which used to be one.
 *
 * "No messages found" was gated on `results.length === 0 && query.trim() &&
 * !searching`. Every one of those is true on the first keystroke, so the panel
 * announced that the archive did not contain a term nobody had searched for
 * yet — a definitive negative answer, produced by the client, about a request
 * the server had never seen. The same verdict then outlived its query: edit a
 * search that found nothing and the message stayed up beneath the new text.
 *
 * Three states have to stay distinguishable here: not searched yet, searched
 * and nothing matched, and searched and the request failed.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const mockSearchMessages = vi.fn();

vi.mock('@/lib/api', () => ({
  searchMessages: (...args: unknown[]) => mockSearchMessages(...args),
  resolveUrl: (u: string) => u,
}));

const HIT = {
  id: 1,
  message_id: 'm1',
  channel_id: 'general',
  server_id: 1,
  sender_pseudonym: 'pseudonym-abcdef',
  content: 'the quick brown fox',
  reply_to_message_id: null,
  created_at: '2026-01-01 00:00:00',
  expires_at: null,
  edited_at: null,
  deleted_at: null,
};

async function openSearch() {
  vi.resetModules();
  const { useIdentityStore } = await import('@/stores/identity');
  const { useChannelsStore } = await import('@/stores/channels');
  const { MessageSearch } = await import('./MessageSearch');

  useIdentityStore.setState({ identity: { pseudonymId: 'p1' } as never });
  useChannelsStore.setState({ activeChannelId: 'general' } as never);

  const user = userEvent.setup();
  render(<MessageSearch />);
  await user.click(screen.getByRole('button', { name: 'Search messages' }));
  return { user, input: screen.getByRole('textbox', { name: 'Search messages' }) };
}

describe('MessageSearch', () => {
  afterEach(() => {
    cleanup();
    vi.clearAllMocks();
  });

  it('does not answer a search that has not run', async () => {
    mockSearchMessages.mockResolvedValue([]);
    const { user, input } = await openSearch();

    await user.type(input, 'fox');

    expect(screen.queryByText('No messages found')).not.toBeInTheDocument();
    expect(mockSearchMessages).not.toHaveBeenCalled();
  });

  it('says nothing matched once the server has actually said so', async () => {
    mockSearchMessages.mockResolvedValue([]);
    const { user, input } = await openSearch();

    await user.type(input, 'fox{Enter}');

    expect(await screen.findByText('No messages found')).toBeInTheDocument();
    expect(mockSearchMessages).toHaveBeenCalledTimes(1);
  });

  it('withdraws that verdict when the query is edited', async () => {
    // The user reads "No messages found" under whatever is in the box. If the
    // box no longer holds the term that was searched, the sentence is false.
    mockSearchMessages.mockResolvedValue([]);
    const { user, input } = await openSearch();

    await user.type(input, 'fox{Enter}');
    expect(await screen.findByText('No messages found')).toBeInTheDocument();

    await user.type(input, 'es');
    expect(screen.queryByText('No messages found')).not.toBeInTheDocument();
  });

  it('withdraws results when the query is edited', async () => {
    mockSearchMessages.mockResolvedValue([HIT]);
    const { user, input } = await openSearch();

    await user.type(input, 'fox{Enter}');
    expect(await screen.findByRole('listbox', { name: 'Search results' })).toBeInTheDocument();

    await user.type(input, 'trot');
    expect(screen.queryByRole('listbox', { name: 'Search results' })).not.toBeInTheDocument();
  });

  it('trims the query, and a whitespace edit does not withdraw the answer', async () => {
    // `handleSearch` searches the trimmed term, so trailing whitespace is not
    // a different query and must not invalidate the answer to it.
    mockSearchMessages.mockResolvedValue([]);
    const { user, input } = await openSearch();

    await user.type(input, '  fox  {Enter}');
    await waitFor(() => expect(mockSearchMessages).toHaveBeenCalled());
    expect(mockSearchMessages.mock.calls[0][1]).toBe('fox');
    expect(await screen.findByText('No messages found')).toBeInTheDocument();

    await user.type(input, ' ');
    expect(screen.getByText('No messages found')).toBeInTheDocument();
  });

  it('distinguishes a failed request from an empty archive', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    mockSearchMessages.mockRejectedValue(new Error('boom'));
    const { user, input } = await openSearch();

    await user.type(input, 'fox{Enter}');

    expect(await screen.findByRole('alert')).toHaveTextContent(/search failed/i);
    expect(screen.queryByText('No messages found')).not.toBeInTheDocument();
  });

  it('re-answers after a retry succeeds', async () => {
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    mockSearchMessages.mockRejectedValueOnce(new Error('boom')).mockResolvedValueOnce([HIT]);
    const { user, input } = await openSearch();

    await user.type(input, 'fox{Enter}');
    await screen.findByRole('alert');

    await user.click(screen.getByRole('button', { name: 'Retry' }));

    expect(await screen.findByRole('listbox', { name: 'Search results' })).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });
});
