/**
 * "Username Visibility" made two claims it had not earned.
 *
 * `loadGrants` and `loadMembers` each ended in a bare `catch` whose whole
 * body was a comment. When either request failed the panel still rendered as
 * if it had the answer:
 *
 *   * a failed member list became **"No other members on this server yet."**
 *     — a statement about the server, produced by a dropped request;
 *   * a failed grant list left `grantees` empty, so every member row read
 *     **"Hidden"** and offered **"Grant"**. That is a privacy assurance: the
 *     user is told nobody can see their username. Worse, because no row is
 *     ever marked granted, the **Revoke** button never appears — a user
 *     opening this dialog specifically to take someone's access away is
 *     told there is nothing to take away, and cannot act.
 *
 * Both must say what happened and offer a retry instead.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const mockListUsernameGrants = vi.fn();
const mockListMembers = vi.fn();

vi.mock('@/lib/api', () => ({
  listUsernameGrants: (...a: unknown[]) => mockListUsernameGrants(...a),
  listMembers: (...a: unknown[]) => mockListMembers(...a),
  setUsername: vi.fn(),
  deleteUsername: vi.fn(),
  grantUsernameVisibility: vi.fn(),
  revokeUsernameVisibility: vi.fn(),
  getVisibleUsernames: vi.fn(async () => ({ usernames: {} })),
}));

vi.mock('@/lib/personas', () => ({
  getPersonasForIdentity: vi.fn(async () => []),
  randomAccentColor: () => '#fff',
  createPersona: vi.fn(),
  updatePersona: vi.fn(),
  deletePersona: vi.fn(),
}));

async function renderPanel() {
  vi.resetModules();
  const { useIdentityStore } = await import('@/stores/identity');
  const { IdentitySettings } = await import('./IdentitySettings');

  useIdentityStore.setState({
    identity: {
      id: 'i1', sk: 'x', pseudonymId: 'p-self', sessionToken: 't', commitmentHex: 'c',
      roleCode: 0, nodeId: 'n', serverSlug: 's', leafIndex: 0, createdAt: '',
    } as never,
  });

  render(<IdentitySettings onClose={() => {}} />);
  return userEvent.setup();
}

describe('username visibility — a failed load is not an answer', () => {
  beforeEach(() => {
    mockListUsernameGrants.mockReset();
    mockListMembers.mockReset();
  });
  afterEach(() => {
    cleanup();
  });

  it('does not report an empty server when the member list fails', async () => {
    mockListUsernameGrants.mockResolvedValue({ grantees: [] });
    mockListMembers.mockRejectedValue(new Error('network down'));

    await renderPanel();

    expect(await screen.findByText(/Could not load the member list/i)).toBeInTheDocument();
    expect(screen.queryByText(/No other members on this server yet/i)).not.toBeInTheDocument();
  });

  it('does not present everyone as Hidden when the grant list fails', async () => {
    mockListUsernameGrants.mockRejectedValue(new Error('network down'));
    mockListMembers.mockResolvedValue([
      { pseudonym_id: 'p-other', participant_type: 'HUMAN' },
    ]);

    await renderPanel();

    expect(await screen.findByText(/Could not load who can see your username/i)).toBeInTheDocument();
    // The roster must not be rendered as authoritative: claiming "Hidden"
    // for a member whose grant state is unknown is the false assurance.
    expect(screen.queryByText(/Hidden/)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Grant' })).not.toBeInTheDocument();
  });

  it('retrying the grant list recovers the real state', async () => {
    mockListMembers.mockResolvedValue([
      { pseudonym_id: 'p-other', participant_type: 'HUMAN' },
    ]);
    mockListUsernameGrants
      .mockRejectedValueOnce(new Error('network down'))
      .mockResolvedValueOnce({ grantees: ['p-other'] });

    const user = await renderPanel();
    await screen.findByText(/Could not load who can see your username/i);

    await user.click(screen.getByRole('button', { name: /retry/i }));

    expect(await screen.findByRole('button', { name: 'Revoke' })).toBeInTheDocument();
  });

  it('still reports a genuinely empty server', async () => {
    mockListUsernameGrants.mockResolvedValue({ grantees: [] });
    mockListMembers.mockResolvedValue([]);

    await renderPanel();

    expect(await screen.findByText(/No other members on this server yet/i)).toBeInTheDocument();
  });
});
