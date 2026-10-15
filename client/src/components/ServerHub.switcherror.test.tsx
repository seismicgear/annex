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
  // MessageBubble subscribes to the attachment grant, so any mock of
  // `@/lib/api` that a message component renders through needs these.
  subscribeUploadGrant: () => () => {},
  getUploadGrantVersion: () => 0,
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

describe('joining a server that will not take you', () => {
  afterEach(() => cleanup());

  it('says why an address was refused rather than "Invalid URL format."', async () => {
    // The second dialog that asks for a server address. `StartupModeSelector`
    // was fixed for this and this one was not, so the same typo produced a
    // useful message on one screen and five useless words on the other.
    const { useServersStore } = await import('@/stores/servers');
    const { ServerHub } = await import('./ServerHub');
    useServersStore.setState({ servers: [SERVER], activeServerId: 's1' } as never);
    render(<ServerHub />);

    await userEvent.click(screen.getByRole('button', { name: /add|join|\+/i }));
    const input = await screen.findByPlaceholderText(/annex\.example\.com|server/i);
    await userEvent.type(input, 'ftp://not-a-web-server');
    await userEvent.click(screen.getByRole('button', { name: 'Join Server' }));

    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('ftp://not-a-web-server');
    expect(alert.textContent).toContain('Only http and https URLs are supported.');
  });
});

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

describe('failed server registration actions', () => {
  afterEach(() => cleanup());

  it('reports a refused removal and leaves the failed registration available', async () => {
    const removeServer = vi.fn().mockRejectedValue(new Error('Local storage is unavailable'));
    await renderHub({ servers: [{ ...SERVER, identityId: '' }], removeServer });
    await userEvent.click(screen.getByTitle('Remove server'));
    expect(await screen.findByRole('alert')).toHaveTextContent(/Could not remove server.*Local storage is unavailable/);
    expect(screen.getByTitle('Remove server')).toBeEnabled();
  });

  it('does not register again when removing the old placeholder failed', async () => {
    const removeServer = vi.fn().mockRejectedValue(new Error('Storage refused removal'));
    const beginRemoteRegistration = vi.fn();
    await renderHub({ servers: [{ ...SERVER, identityId: '' }], removeServer, beginRemoteRegistration });
    await userEvent.click(screen.getByRole('button', { name: 'Retry registration' }));
    expect(await screen.findByRole('alert')).toHaveTextContent(/Could not retry Alpha.*Storage refused removal/);
    expect(beginRemoteRegistration).not.toHaveBeenCalled();
  });

  it('reports a new registration failure and allows retrying from the main server icon', async () => {
    const removeServer = vi.fn().mockResolvedValue(undefined);
    const beginRemoteRegistration = vi.fn().mockResolvedValue(null);
    await renderHub({
      servers: [{ ...SERVER, identityId: '' }], removeServer, beginRemoteRegistration,
      registrationError: 'Remote server is offline',
    });
    await userEvent.click(screen.getByTitle('Alpha — registration failed (click to retry)'));
    expect(await screen.findByRole('alert')).toHaveTextContent(/Could not retry Alpha.*Remote server is offline/);
    expect(beginRemoteRegistration).toHaveBeenCalledWith(SERVER.baseUrl);
    expect(screen.getByRole('button', { name: 'Retry registration' })).toBeEnabled();
  });
});
