import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { StartupGate, type StartupGateProps } from './StartupGate';

vi.mock('@/components/StartupModeSelector', () => ({ StartupModeSelector: () => <div>Saved startup mode resumed</div> }));
vi.mock('@/components/IdentitySetup', () => ({ IdentitySetup: () => <div>Create your identity</div> }));
vi.mock('@/stores/identity', () => ({ useIdentityStore: {} }));
vi.mock('@/stores/servers', () => ({ useServersStore: {} }));
vi.mock('@/lib/api', () => ({ setSessionToken: vi.fn() }));
vi.mock('@/lib/db', () => ({ clearAllDatabases: vi.fn() }));
vi.mock('@/lib/tauri', () => ({ clearStartupMode: vi.fn(), isTauri: () => true, resetServerData: vi.fn() }));
const props: StartupGateProps = {
  identityChecked: true, startupInitError: null, startupErrorDetails: null, errorDetails: null,
  phase: 'keys_ready', error: null, identity: { id: 'identity', sk: '1234', commitmentHex: 'abcd',
    roleCode: 1, nodeId: 1, createdAt: '', serverSlug: '', pseudonymId: null, sessionToken: null, leafIndex: null },
  pendingInvite: null, pendingProtocolInvite: null, pendingProtocolInviteConfirmation: null,
  handleAcceptProtocolInvite: vi.fn(async () => {}), handleIgnoreProtocolInvite: vi.fn(),
  serverReady: false, passwordRequired: false, setServerPassword: vi.fn(), proofInFlight: false,
  provingStatus: 'idle', provingFailures: 0, beginStartupFlow: () => 1, setServerReady: vi.fn(),
  setDegradedStartup: vi.fn(), setPasswordRequired: vi.fn(), resetToServerSelection: vi.fn(async () => {}), retryBootstrap: vi.fn(),
};
afterEach(cleanup);
describe('invite priority over saved startup mode', () => {
  it('does not mount the auto-resuming selector while the native inbox is loading', () => {
    render(<StartupGate {...props} protocolInviteLoading />);
    expect(screen.getByRole('status')).toHaveTextContent('Checking for an invite');
    expect(screen.queryByText('Saved startup mode resumed')).not.toBeInTheDocument();
  });
  it('waits for explicit Join or Ignore before exposing a competing startup mode', () => {
    const invite = { server: 'https://destination.example', code: 'private-code' };
    const view = render(<StartupGate {...props} pendingProtocolInviteConfirmation={invite} />);
    expect(screen.getByRole('button', { name: 'Join invite server' })).toBeVisible();
    expect(screen.queryByText('Saved startup mode resumed')).not.toBeInTheDocument();
    view.rerender(<StartupGate {...props} />);
    expect(screen.getByText('Saved startup mode resumed')).toBeVisible();
  });
  it('preserves the pending destination while the user creates their offline identity', () => {
    render(<StartupGate {...props} identity={null} phase="uninitialized"
      pendingProtocolInvite={{ server: 'https://destination.example', code: 'private-code' }} />);
    expect(screen.getByText('Create your identity')).toBeVisible();
    expect(screen.getByText('Invite received — create your identity to continue.')).toBeVisible();
    expect(screen.queryByText('Saved startup mode resumed')).not.toBeInTheDocument();
  });
});
