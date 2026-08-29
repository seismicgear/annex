/**
 * Being unable to read an encrypted channel had no representation at all.
 *
 * `resolveChannelKey` distinguishes two outcomes precisely — an
 * `E2eKeyPendingError` means the channel is keyed and we simply have not been
 * admitted yet, anything else is a real failure — and `ensureChannelReady`
 * threw both away in one bare `catch`. So both looked the same from the UI:
 * a column of "🔒 encrypted message (no key)" beneath a status bar reading
 * "End-to-end encrypted — the server can't read these messages." True, and
 * useless. Nothing said why the messages were unreadable, that the wait ends
 * by itself, or that anything had gone wrong.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';

vi.mock('@/lib/api', () => ({
  setChannelE2e: vi.fn(),
  getVisibleUsernames: vi.fn(async () => ({ usernames: {} })),
}));

async function renderBar(overrides: Record<string, unknown>) {
  vi.resetModules();
  const { useChannelsStore } = await import('@/stores/channels');
  const { useIdentityStore } = await import('@/stores/identity');
  const { ChannelEncryptionBar } = await import('./ChannelEncryptionBar');

  useIdentityStore.setState({
    identity: { id: 'i1', pseudonymId: 'p-self' } as never,
    permissions: { capabilities: { can_moderate: false } } as never,
  });
  useChannelsStore.setState({
    activeChannelId: 'ch-1',
    activeChannelE2e: true,
    activeChannelKeyState: 'ready',
    activeChannelKeyError: null,
    ...overrides,
  } as never);

  render(<ChannelEncryptionBar />);
  return useChannelsStore;
}

describe('channel encryption bar — the two ways an encrypted channel is unreadable', () => {
  beforeEach(() => vi.clearAllMocks());
  afterEach(() => cleanup());

  it('does not claim all is well while awaiting admission', async () => {
    await renderBar({ activeChannelKeyState: 'pending' });

    expect(
      screen.queryByText(/the server can't read these messages/i),
    ).not.toBeInTheDocument();
    expect(screen.getByText(/don't have this channel's key yet/i)).toBeInTheDocument();
    // The wait ends on its own — `reconcile` admits every current member on
    // channel open — and saying so is the difference between a state and a
    // dead end.
    expect(screen.getByText(/automatically the next time they open/i)).toBeInTheDocument();
  });

  it('reports a failed key resolution as a failure, with its reason', async () => {
    await renderBar({
      activeChannelKeyState: 'failed',
      activeChannelKeyError: 'network down',
    });

    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent(/could not be loaded/i);
    expect(alert).toHaveTextContent(/network down/);
    expect(
      screen.queryByText(/the server can't read these messages/i),
    ).not.toBeInTheDocument();
  });

  it('pending and failed are not the same notice', async () => {
    await renderBar({ activeChannelKeyState: 'pending' });
    // `pending` resolves by itself, so it must not be an alert.
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    expect(screen.getByRole('status')).toBeInTheDocument();
  });

  it('still reassures when the key really is held', async () => {
    await renderBar({ activeChannelKeyState: 'ready' });
    expect(
      screen.getByText(/the server can't read these messages/i),
    ).toBeInTheDocument();
  });
});
