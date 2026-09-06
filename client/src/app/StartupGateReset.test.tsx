/**
 * The "Clear local state" escape hatch.
 *
 * It is the strongest recovery the startup screen offers, shown only when
 * bootstrap has already failed. It cleared IndexedDB and the server data
 * directory but left the saved startup mode in place — so re-bootstrapping
 * auto-resumed the mode that had just failed, which is often the cause (a
 * saved "use this server" pointing at a host that is gone). The milder Retry
 * button beside it has always cleared the mode, for exactly that reason.
 */

import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const clearAllDatabases = vi.fn(async () => ({ unremoved: [] as string[] }));
vi.mock('@/lib/db', () => ({
  clearAllDatabases: () => clearAllDatabases(),
}));

const resetServerData = vi.fn(async () => {});
const clearStartupMode = vi.fn(async () => {});
const isTauri = vi.fn(() => true);
vi.mock('@/lib/tauri', () => ({
  resetServerData: () => resetServerData(),
  clearStartupMode: () => clearStartupMode(),
  isTauri: () => isTauri(),
}));

const clearWebStartupMode = vi.fn();
vi.mock('@/lib/startup-prefs', () => ({
  clearWebStartupMode: () => clearWebStartupMode(),
}));

vi.mock('@/lib/api', () => ({ setSessionToken: vi.fn() }));

import { StartupGate, type StartupGateProps } from './StartupGate';

function renderFailedStartup(overrides: Partial<StartupGateProps> = {}) {
  const retryBootstrap = vi.fn();
  const props: StartupGateProps = {
    identityChecked: true,
    // The branch that renders the escape hatch: startup failed before any
    // identity key existed.
    startupInitError: 'could not reach the server',
    startupErrorDetails: null,
    errorDetails: null,
    phase: 'error',
    error: 'could not reach the server',
    identity: null,
    pendingInvite: null,
    pendingProtocolInvite: null,
    pendingProtocolInviteConfirmation: null,
    handleAcceptProtocolInvite: async () => {},
    handleIgnoreProtocolInvite: () => {},
    serverReady: false,
    passwordRequired: false,
    setServerPassword: vi.fn(),
    proofInFlight: false,
    provingStatus: 'idle',
    provingFailures: 0,
    beginStartupFlow: () => 1,
    setServerReady: vi.fn(),
    setDegradedStartup: vi.fn(),
    setPasswordRequired: vi.fn(),
    resetToServerSelection: vi.fn(async () => {}),
    retryBootstrap,
    ...overrides,
  };
  render(<StartupGate {...props} />);
  return { retryBootstrap };
}

describe('Clear local state', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    clearAllDatabases.mockResolvedValue({ unremoved: [] });
    isTauri.mockReturnValue(true);
  });

  it('clears the saved startup mode along with the data', async () => {
    const user = userEvent.setup();
    const { retryBootstrap } = renderFailedStartup();

    await user.click(screen.getByRole('button', { name: 'Clear local state' }));

    await waitFor(() => expect(retryBootstrap).toHaveBeenCalled());
    expect(clearAllDatabases).toHaveBeenCalled();
    expect(resetServerData).toHaveBeenCalled();
    // Without these, the re-bootstrap resumes the mode that just failed.
    expect(clearStartupMode).toHaveBeenCalled();
    expect(clearWebStartupMode).toHaveBeenCalled();
  });

  it('clears only the web preference outside Tauri', async () => {
    isTauri.mockReturnValue(false);
    const user = userEvent.setup();
    renderFailedStartup();

    await user.click(screen.getByRole('button', { name: 'Clear local state' }));

    await waitFor(() => expect(clearWebStartupMode).toHaveBeenCalled());
    expect(clearStartupMode).not.toHaveBeenCalled();
  });

  it('does not re-bootstrap when the data could not be cleared', async () => {
    // The escape hatch used to run the reset-and-retry unconditionally, which
    // is what made it look like it worked: the stores were emptied, bootstrap
    // re-ran, and it reloaded from the IndexedDB that had never been deleted.
    clearAllDatabases.mockResolvedValue({ unremoved: ['annex-identities'] });
    const user = userEvent.setup();
    const { retryBootstrap } = renderFailedStartup();

    await user.click(screen.getByRole('button', { name: 'Clear local state' }));

    await waitFor(() =>
      expect(screen.getByText(/Some local data could not be cleared/)).toBeInTheDocument(),
    );
    expect(retryBootstrap).not.toHaveBeenCalled();
  });
});
