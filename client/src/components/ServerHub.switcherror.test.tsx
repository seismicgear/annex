/**
 * A failed server switch used to be a single unstyled "!".
 *
 * `<div className="server-hub-error" role="alert" title={switchError}>` around
 * `<span>!</span>`: the live region announced the character "!", the sentence
 * explaining the failure lived only in a `title` — hover-only on a pointer,
 * unreachable on touch — and `.server-hub-error` had no CSS rule anywhere in
 * the stylesheet, so it rendered as bare text in the icon rail. Nothing said
 * the switch had rolled back and the user was still on the server they
 * started from, which is the one fact that decides what they do next.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('@/lib/api', () => ({
  resolveUrl: (u: string) => u,
  setApiBaseUrl: vi.fn(),
}));

const SERVER = {
  id: 's1',
  slug: 'alpha',
  label: 'Alpha',
  baseUrl: 'https://alpha.example',
  identityId: 'i1',
};

async function renderHub(overrides: Record<string, unknown>) {
  vi.resetModules();
  const { useServersStore } = await import('@/stores/servers');
  const { ServerHub } = await import('./ServerHub');

  useServersStore.setState({
    servers: [SERVER],
    activeServerId: 's1',
    switching: false,
    switchError: null,
    serverImageUrl: null,
    ...overrides,
  } as never);

  render(<ServerHub />);
  return useServersStore;
}

describe('server switch failure', () => {
  afterEach(() => cleanup());

  it('announces the reason as text, not as a tooltip', async () => {
    await renderHub({ switchError: 'Server returned 503' });

    const alert = screen.getByRole('alert');
    // The assertion that would have failed before: the reason has to be IN
    // the live region, because that is what a screen reader reads out and
    // what a touch user can see.
    expect(alert.textContent).toContain('Server returned 503');
    expect(alert).not.toHaveAttribute('title');
  });

  it('says the switch rolled back', async () => {
    await renderHub({ switchError: 'Failed to fetch' });

    expect(screen.getByRole('alert').textContent).toMatch(
      /still on the server you started from/i,
    );
  });

  it('is dismissible', async () => {
    const store = await renderHub({ switchError: 'Failed to fetch' });

    await userEvent.click(screen.getByRole('button', { name: /dismiss/i }));

    expect(store.getState().switchError).toBeNull();
    expect(screen.queryByRole('alert')).toBeNull();
  });

  it('shows nothing when the switch succeeded', async () => {
    await renderHub({ switchError: null });

    expect(screen.queryByRole('alert')).toBeNull();
  });
});
