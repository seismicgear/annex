/**
 * The password prompt for `access_mode: password` servers.
 *
 * These pin the two halves of a defect that made password-protected servers
 * unusable: the submit control did nothing, and the value reached the
 * registration effect on every keystroke instead of on submit.
 */

import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StartupGate, type StartupGateProps } from './StartupGate';

function renderPrompt(overrides: Partial<StartupGateProps> = {}) {
  const setServerPassword = vi.fn();
  const setPasswordRequired = vi.fn();
  const resetToServerSelection = vi.fn(async () => {});

  const props: StartupGateProps = {
    identityChecked: true,
    startupInitError: null,
    startupErrorDetails: null,
    errorDetails: null,
    phase: 'keys_ready',
    error: null,
    identity: { sk: 'deadbeef', pseudonymId: null } as StartupGateProps['identity'],
    pendingInvite: null,
    pendingProtocolInvite: null,
    pendingProtocolInviteConfirmation: null,
    handleAcceptProtocolInvite: async () => {},
    handleIgnoreProtocolInvite: () => {},
    serverReady: true,
    passwordRequired: true,
    setServerPassword,
    proofInFlight: false,
    provingStatus: 'idle',
    provingFailures: 0,
    beginStartupFlow: () => 1,
    setServerReady: vi.fn(),
    setDegradedStartup: vi.fn(),
    setPasswordRequired,
    resetToServerSelection,
    retryBootstrap: vi.fn(),
    ...overrides,
  };

  render(<StartupGate {...props} />);
  return { setServerPassword, setPasswordRequired, resetToServerSelection };
}

describe('server password prompt', () => {
  it('does not submit anything while the user is still typing', async () => {
    // The input was bound straight to the parent's `serverPassword`, which the
    // registration effect depends on — so typing "hunter2" fired seven
    // registration attempts, each with a partial password, against a
    // ten-per-minute budget.
    const user = userEvent.setup();
    const { setServerPassword } = renderPrompt();

    await user.type(screen.getByPlaceholderText('Enter server password'), 'hunter2');

    expect(setServerPassword).not.toHaveBeenCalled();
  });

  it('submits exactly once when the button is clicked', async () => {
    // The button's onClick body used to be only a comment, so the primary
    // call to action did nothing at all.
    const user = userEvent.setup();
    const { setServerPassword } = renderPrompt();

    await user.type(screen.getByPlaceholderText('Enter server password'), 'hunter2');
    await user.click(screen.getByRole('button', { name: 'Join Server' }));

    expect(setServerPassword).toHaveBeenCalledTimes(1);
    expect(setServerPassword).toHaveBeenCalledWith('hunter2');
  });

  it('submits on Enter as well as on click', async () => {
    const user = userEvent.setup();
    const { setServerPassword } = renderPrompt();

    await user.type(screen.getByPlaceholderText('Enter server password'), 'hunter2{Enter}');

    expect(setServerPassword).toHaveBeenCalledWith('hunter2');
  });

  it('keeps the submit disabled until something has been typed', async () => {
    const user = userEvent.setup();
    renderPrompt();

    const join = screen.getByRole('button', { name: 'Join Server' });
    expect(join).toBeDisabled();

    await user.type(screen.getByPlaceholderText('Enter server password'), '   ');
    expect(join, 'whitespace is not a password').toBeDisabled();

    await user.type(screen.getByPlaceholderText('Enter server password'), 'x');
    expect(join).toBeEnabled();
  });

  it('goes back instead of submitting when Back is pressed', async () => {
    // "Back" had no `type`, so inside a form it defaulted to submit.
    const user = userEvent.setup();
    const { setServerPassword, setPasswordRequired, resetToServerSelection } = renderPrompt();

    await user.type(screen.getByPlaceholderText('Enter server password'), 'hunter2');
    await user.click(screen.getByRole('button', { name: 'Back' }));

    expect(setPasswordRequired).toHaveBeenCalledWith(false);
    expect(resetToServerSelection).toHaveBeenCalled();
    expect(setServerPassword).not.toHaveBeenCalled();
  });

  it('labels the password field for assistive technology', () => {
    renderPrompt();
    expect(screen.getByLabelText('Server password')).toBeInTheDocument();
  });
});
