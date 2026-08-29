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
import { useEffect, useState } from 'react';
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

/**
 * Password prompt for a server whose `access_mode` is `password`.
 *
 * The typed value is LOCAL state, and only reaches the parent on submit.
 * That separation is the whole point of this component: the registration
 * effect in `useServerSelection` keys off the parent's `serverPassword`, so
 * while the input was bound directly to it, every keystroke re-ran the effect
 * and fired a registration attempt with the partial password typed so far.
 * Typing "hunter2" meant seven attempts against a ten-per-minute registration
 * budget, none of them the password the user meant to send — and the "Join
 * Server" button, whose `onClick` body was only a comment, did nothing at all.
 */
function PasswordPrompt({
  onSubmit,
  onBack,
}: {
  onSubmit: (password: string) => void;
  onBack: () => void;
}) {
  const [draft, setDraft] = useState('');
  const trimmed = draft.trim();

  return (
    <div className="password-prompt">
      <p>This server requires a password to join.</p>
      <form
        onSubmit={(e) => {
          e.preventDefault();
          if (trimmed) onSubmit(trimmed);
        }}
      >
        <label className="visually-hidden" htmlFor="server-password">
          Server password
        </label>
        <input
          id="server-password"
          type="password"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="Enter server password"
          autoComplete="current-password"
          autoFocus
        />
        {/* `type` is explicit on both: without it a button inside a form
            defaults to submit, so "Back" was submitting the form. */}
        <button type="submit" className="primary-btn" disabled={!trimmed}>
          Join Server
        </button>
        <button type="button" className="secondary-btn" onClick={onBack}>
          Back
        </button>
      </form>
    </div>
  );
}

export function StartupGate(props: StartupGateProps) {
  const [clearStateError, setClearStateError] = useState<string | null>(null);
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

  // Elapsed-time counter for the active registration/proof phases. A first-run
  // Groth16 proof can take 30–60s; without a moving indicator the static label
  // looked frozen (AUDIT P4-UX-1). The timer runs continuously while any
  // working phase is active and resets when we leave them.
  const isWorking =
    phase === 'keys_ready' ||
    phase === 'registering' ||
    phase === 'proving' ||
    phase === 'verifying';
  // Derived rather than reset. Writing `setElapsed(0)` synchronously in the
  // effect is the cascading-render pattern `react-hooks/set-state-in-effect`
  // exists to catch; rendering 0 whenever the work is not running says the
  // same thing without a state write.
  const [tick, setTick] = useState(0);
  const elapsed = isWorking ? tick : 0;
  useEffect(() => {
    if (!isWorking) return;
    const started = Date.now();
    const id = setInterval(() => {
      setTick(Math.floor((Date.now() - started) / 1000));
    }, 1000);
    return () => clearInterval(id);
  }, [isWorking]);

  // Gate 0: Still checking IndexedDB for existing identities.
  if (!identityChecked) {
    return (
      <div className="app">
        <main className="app-main setup">
          <div className="startup-mode-selector">
            <h1>Annex</h1>
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
            <h1>Annex</h1>
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
            {clearStateError && (
              <div className="clear-state-error" role="alert">
                {clearStateError}
              </div>
            )}
            <button
              className="secondary-btn"
              onClick={() => {
                void (async () => {
                  // No `finally`: the reset-and-retry must run only when the
                  // data is actually gone. It used to run unconditionally,
                  // which is what made this button appear to work — the
                  // in-memory stores were cleared, bootstrap re-ran, and it
                  // reloaded the identities from the IndexedDB that had never
                  // been deleted, landing the user back in the same failure
                  // with nothing said. This is the escape hatch; failing it
                  // silently leaves no way forward.
                  let cleared = false;
                  try {
                    const { unremoved } = await clearAllDatabases();
                    await resetServerData();
                    if (unremoved.length > 0) {
                      // Almost always another Annex tab holding the database
                      // open, which the user can act on once told.
                      setClearStateError(
                        'Some local data could not be cleared. Close any other Annex tabs or windows, then try again.',
                      );
                    } else {
                      cleared = true;
                    }
                  } catch (e) {
                    console.warn('clear local state failed:', e);
                    setClearStateError(
                      'Could not clear local data. Close any other Annex tabs or windows, then try again.',
                    );
                  }

                  if (!cleared) return;

                  setClearStateError(null);
                  useServersStore.setState({ servers: [], activeServerId: null, serverImageUrl: null, pendingRegistrationServerId: null, switchError: null });
                  useIdentityStore.setState({ identity: null, phase: 'uninitialized', error: null, errorDetails: null });
                  setSessionToken(null);
                  retryBootstrap();
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
          <div className="invite-confirmation-banner" role="region" aria-live="polite" aria-label="Invite confirmation">
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
        {pendingProtocolInviteConfirmation && (
          <div className="invite-confirmation-banner" role="region" aria-live="polite" aria-label="Invite confirmation">
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
        {pendingProtocolInviteConfirmation && (
          <div className="invite-confirmation-banner" role="region" aria-live="polite" aria-label="Invite confirmation">
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
        <div className="identity-setup">
          <h1>Annex</h1>
          {passwordRequired && phase === 'keys_ready' ? (
            <PasswordPrompt
              onSubmit={setServerPassword}
              onBack={() => {
                setPasswordRequired(false);
                void resetToServerSelection();
              }}
            />
          ) : (
            <div className={`phase-status phase-${phase}`}>
              <span className="startup-spinner" aria-hidden="true" />
              <span className="phase-label">
                {phase === 'proving'
                  ? PROVING_STATUS_LABELS[provingStatus]
                  : (REGISTRATION_LABELS[phase] ?? 'Preparing...')}
              </span>
              {elapsed >= 3 && (
                <span className="phase-elapsed">
                  {elapsed}s elapsed · the first proof can take 30–60s on slower
                  hardware
                </span>
              )}
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
