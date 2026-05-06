/**
 * Pre-main-app render gates. Evaluated top-to-bottom; first match wins.
 *
 *   Gate 0   – identity check still in flight (loading splash)
 *   Gate 0.5 – fatal startup-init error with retry / clear-state controls
 *   Gate 1   – no identity keys (IdentitySetup)
 *   Gate 2   – keys exist but no server picked (StartupModeSelector)
 *   Gate 3   – registering / proving / password prompt / registration error
 *
 * AppShell only renders this when at least one gate matches; otherwise it
 * renders MainLayout.
 */

import type { Dispatch, SetStateAction } from 'react';
import { IdentitySetup } from '@/components/IdentitySetup';
import { StartupModeSelector, type DegradedStartupInfo } from '@/components/StartupModeSelector';
import { setSessionToken } from '@/lib/api';
import { clearAllDatabases } from '@/lib/db';
import { resetServerData } from '@/lib/tauri';
import { useIdentityStore } from '@/stores/identity';
import { useServersStore } from '@/stores/servers';
import type { IdentityPhase, ProvingStatus } from '@/stores/identity';
import type { InvitePayload, LegacyInvitePayload, StoredIdentity } from '@/types';

/** Labels shown while registering keys with the chosen server. */
const REGISTRATION_LABELS: Record<string, string> = {
  keys_ready: 'Preparing to register...',
  registering: 'Registering with server...',
  proving: 'Generating zero-knowledge proof...',
  verifying: 'Verifying membership...',
};

const PROVING_STATUS_LABELS: Record<ProvingStatus, string> = {
  idle: 'Generating zero-knowledge proof...',
  loading_assets: 'Loading proof assets...',
  computing_witness: 'Computing witness...',
  generating_proof: 'Generating proof...',
};

export interface StartupGateProps {
  identityChecked: boolean;
  startupInitError: string | null;
  startupErrorDetails: string | null;
  errorDetails: string | null;
  phase: IdentityPhase;
  error: string | null;
  identity: StoredIdentity | null;
  pendingInvite: LegacyInvitePayload | null;
  pendingProtocolInvite: InvitePayload | null;
  pendingProtocolInviteConfirmation: InvitePayload | null;
  handleAcceptProtocolInvite: () => Promise<void>;
  handleIgnoreProtocolInvite: () => void;
  serverReady: boolean;
  passwordRequired: boolean;
  serverPassword: string;
  setServerPassword: Dispatch<SetStateAction<string>>;
  proofInFlight: boolean;
  provingStatus: ProvingStatus;
  provingFailures: number;
  beginStartupFlow: () => number;
  setServerReady: Dispatch<SetStateAction<boolean>>;
  setDegradedStartup: Dispatch<SetStateAction<DegradedStartupInfo | null>>;
  setPasswordRequired: Dispatch<SetStateAction<boolean>>;
  resetToServerSelection: () => Promise<void>;
  retryBootstrap: () => void;
}

export function StartupGate(props: StartupGateProps) {
  const {
    identityChecked,
    startupInitError,
    startupErrorDetails,
    errorDetails,
    phase,
    error,
    identity,
    pendingInvite,
    pendingProtocolInvite,
    pendingProtocolInviteConfirmation,
    handleAcceptProtocolInvite,
    handleIgnoreProtocolInvite,
    serverReady,
    passwordRequired,
    serverPassword,
    setServerPassword,
    proofInFlight,
    provingStatus,
    provingFailures,
    beginStartupFlow,
    setServerReady,
    setDegradedStartup,
    setPasswordRequired,
    resetToServerSelection,
    retryBootstrap,
  } = props;

  // Gate 0: Still checking IndexedDB for existing identities.
  if (!identityChecked) {
    return (
      <div className="app">
        <main className="app-main setup">
          <div className="startup-mode-selector">
            <h2>Annex</h2>
            <div className="startup-loading">Loading...</div>
          </div>
        </main>
      </div>
    );
  }

  if (startupInitError && phase === 'error' && !identity?.sk) {
    return (
      <div className="app">
        <main className="app-main setup">
          <div className="startup-mode-selector">
            <h2>Annex</h2>
            <div className="error-message">Startup failed: {startupInitError}</div>
            {(startupErrorDetails || errorDetails) && (
              <details className="error-details">
                <summary>Details</summary>
                <pre>{startupErrorDetails ?? errorDetails}</pre>
              </details>
            )}
            <button className="primary-btn" onClick={retryBootstrap}>
              Retry startup
            </button>
            <button
              className="secondary-btn"
              onClick={() => {
                void (async () => {
                  try {
                    await clearAllDatabases();
                    await resetServerData();
                  } catch (e) {
                    console.warn('clear local state failed:', e);
                  } finally {
                    useServersStore.setState({ servers: [], activeServerId: null, serverImageUrl: null, pendingRegistrationServerId: null, switchError: null });
                    useIdentityStore.setState({ identity: null, phase: 'uninitialized', error: null, errorDetails: null });
                    setSessionToken(null);
                    retryBootstrap();
                  }
                })();
              }}
            >
              Clear local state
            </button>
          </div>
        </main>
      </div>
    );
  }

  // Gate 1 — HARD GATE: No identity keys → Screen 1 (identity creation).
  // This screen makes ZERO network requests.
  if (!identity?.sk) {
    return (
      <div className="app">
        <header className="app-header">
          <h1>Annex</h1>
          {pendingInvite && (
            <span className="invite-banner">
              Joining {pendingInvite.label ?? pendingInvite.channelId}...
            </span>
          )}
          {pendingProtocolInvite && (
            <span className="invite-banner">
              Invite received — create your identity to continue.
            </span>
          )}
        </header>
        {pendingProtocolInviteConfirmation && (
          <div className="invite-confirmation-banner" role="dialog" aria-label="Invite confirmation">
            <span>
              Invite received for {pendingProtocolInviteConfirmation.server}
            </span>
            <button className="primary-btn" onClick={handleAcceptProtocolInvite}>
              Join invite server
            </button>
            <button className="secondary-btn" onClick={handleIgnoreProtocolInvite}>
              Ignore
            </button>
          </div>
        )}
        <main className="app-main setup">
          <IdentitySetup />
        </main>
      </div>
    );
  }

  // Gate 2: Server not yet selected → Screen 2 (startup mode selector).
  // The server is NOT started yet — StartupModeSelector handles starting
  // the embedded server if the user picks "Host a Server".
  if (!serverReady) {
    return (
      <div className="app">
        <main className="app-main setup">
          <StartupModeSelector
            onReady={(degraded) => {
              beginStartupFlow();
              setServerReady(true);
              if (degraded) setDegradedStartup(degraded);
            }}
          />
        </main>
      </div>
    );
  }

  // Gate 3: Identity keys not yet registered with the chosen server.
  // The auto-register effect handles the registration automatically;
  // this gate just shows progress while it runs.
  return (
    <div className="app">
      <main className="app-main setup">
        <div className="identity-setup">
          <h2>Annex</h2>
          {passwordRequired && phase === 'keys_ready' ? (
            <div className="password-prompt">
              <p>This server requires a password to join.</p>
              <form onSubmit={(e) => { e.preventDefault(); }}>
                <input
                  type="password"
                  value={serverPassword}
                  onChange={(e) => setServerPassword(e.target.value)}
                  placeholder="Enter server password"
                  autoFocus
                />
                <button
                  className="primary-btn"
                  disabled={!serverPassword.trim()}
                  onClick={() => {
                    // Trigger re-run of the auto-register effect
                    // by updating serverPassword (already in deps)
                  }}
                >
                  Join Server
                </button>
                <button
                  className="secondary-btn"
                  onClick={() => { setPasswordRequired(false); void resetToServerSelection(); }}
                >
                  Back
                </button>
              </form>
            </div>
          ) : (
            <div className={`phase-status phase-${phase}`}>
              {phase === 'proving'
                ? PROVING_STATUS_LABELS[provingStatus]
                : (REGISTRATION_LABELS[phase] ?? 'Preparing...')}
            </div>
          )}
          {phase === 'error' && error && (
            <>
              <div className="error-message">{error}</div>
              {error.includes('Proof generation timed out') && (
                <div className="error-message">Hint: the first proof can take longer on slower hardware.</div>
              )}
              {(startupErrorDetails || errorDetails) && (
                <details className="error-details">
                  <summary>Details</summary>
                  <pre>{startupErrorDetails ?? errorDetails}</pre>
                </details>
              )}
              {proofInFlight && (
                <div className="error-message">proof still running.</div>
              )}
              <button
                className="primary-btn"
                onClick={() => { void resetToServerSelection(); }}
              >
                {proofInFlight ? 'Retry (cancel running proof)' : 'Retry'}
              </button>
              {provingFailures >= 2 && (
                <button
                  className="secondary-btn"
                  onClick={() => { void resetToServerSelection(); }}
                >
                  Back to server selection
                </button>
              )}
            </>
          )}
        </div>
      </main>
    </div>
  );
}
