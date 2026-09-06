/**
 * Drive the cold-start bootstrap: load identities, run the Tauri
 * fresh-install cleanup when needed, and load the saved server list.
 *
 * Also owns the startup-error mirror state (so a phase=error transition
 * can capture diagnostic details for the gate UI) and the proof-in-flight
 * sync poll that clears stale "proof generating" status from the store.
 */

import { useEffect, useState } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useServersStore } from '@/stores/servers';
import { setSessionToken } from '@/lib/api';
import { clearAllDatabases } from '@/lib/db';
import { resetServerData, checkFirstRunCompleted } from '@/lib/tauri';
import { isProofGenerationInFlight } from '@/lib/zk';
import type { IdentityPhase } from '@/stores/identity';

interface UseAppBootstrapArgs {
  phase: IdentityPhase;
  error: string | null;
  errorDetails: string | null;
  proofInFlight: boolean;
  loadIdentities: () => Promise<void>;
  loadServers: () => Promise<void>;
  inTauri: boolean;
}

interface UseAppBootstrapResult {
  identityChecked: boolean;
  startupInitError: string | null;
  startupErrorDetails: string | null;
  setStartupErrorDetails: React.Dispatch<React.SetStateAction<string | null>>;
  provingFailures: number;
  setProvingFailures: React.Dispatch<React.SetStateAction<number>>;
  retryBootstrap: () => void;
}

export function useAppBootstrap({
  phase,
  error,
  errorDetails,
  proofInFlight,
  loadIdentities,
  loadServers,
  inTauri,
}: UseAppBootstrapArgs): UseAppBootstrapResult {
  const [identityChecked, setIdentityChecked] = useState(false);
  const [startupInitError, setStartupInitError] = useState<string | null>(null);
  const [startupErrorDetails, setStartupErrorDetails] = useState<string | null>(null);
  const [bootstrapAttempt, setBootstrapAttempt] = useState(0);
  const [provingFailures, setProvingFailures] = useState(0);

  // Capture diagnostic details and proving-failure counts when phase enters error.
  useEffect(() => {
    if (phase !== 'error') return;

    if (errorDetails) {
      setStartupErrorDetails((prev) => prev ?? errorDetails);
    }

    if (error?.includes('Proof assets missing') || error?.includes('Proof generation timed out')) {
      setProvingFailures((count) => count + 1);
    }
  }, [phase, error, errorDetails]);

  // Reconcile store-side proofInFlight with the actual ZK in-flight flag.
  useEffect(() => {
    if (!proofInFlight) return;

    const syncProofFlightState = () => {
      if (!isProofGenerationInFlight()) {
        useIdentityStore.setState({ proofInFlight: false, provingStatus: 'idle' });
      }
    };

    syncProofFlightState();
    const interval = setInterval(syncProofFlightState, 400);
    return () => clearInterval(interval);
  }, [proofInFlight]);

  // ── Load identities + servers on mount (all modes) ──
  // In Tauri mode, after loading identities we check the dedicated
  // first_run_completed marker (NOT startup_prefs.json). If the marker
  // is absent the user has never completed initial setup — reset identity
  // selection so IdentitySetup renders first, even if IndexedDB has a
  // valid identity from a previous install. This avoids the old bug where
  // logout (which clears startup_prefs.json) would trigger destructive
  // cleanup on next launch.
  useEffect(() => {
    const bootstrap = async () => {
      setStartupInitError(null);
      try {
        await loadIdentities();
        const firstRunDone = inTauri ? await checkFirstRunCompleted().catch(() => true) : undefined;

        // Existing identities are direct evidence that this is NOT a fresh
        // install, and they outrank a missing marker file.
        //
        // The marker is written by `markFirstRunCompleted()`, whose failure
        // is swallowed at its call site. One failed write — a full disk, a
        // permissions problem, an IPC hiccup — and the marker never appears,
        // so the NEXT launch takes this branch and destroys every identity,
        // server, persona, message and upload the user has. Silently, and
        // irreversibly, because absence of a marker was being treated as
        // proof of a fresh install.
        //
        // The check above already fails safe (`.catch(() => true)`). This
        // makes the decision itself fail safe: wiping is only defensible
        // when there is demonstrably nothing to lose. A genuinely fresh
        // install has no identities, so it still gets its cleanup.
        const hasExistingIdentities = useIdentityStore.getState().storedIdentities.length > 0;
        if (inTauri && firstRunDone === false && hasExistingIdentities) {
          console.warn(
            'first-run marker missing but identities exist — skipping destructive cleanup',
          );
        }
        if (inTauri && firstRunDone === false && !hasExistingIdentities) {
          // Fresh install detected (no first_run_completed marker). Clear
          // ALL stale data from a previous installation so the user starts
          // clean:
          //   1. IndexedDB databases (identities, servers, personas)
          //   2. Server data directory (database, uploads, config)
          // Without this, old identities persist in the server DB and the
          // new identity won't be recognised as the server founder/admin.
          try {
            await clearAllDatabases();
            await resetServerData();
          } catch (e) {
            console.warn('fresh install cleanup failed (non-fatal):', e);
          }
          // Clear in-memory server state so the UI reflects the emptied IndexedDB.
          useServersStore.setState({
            servers: [],
            activeServerId: null,
            serverImageUrl: null,
            pendingRegistrationServerId: null,
            switchError: null,
          });
          // Re-load servers from the now-empty IndexedDB so the store is consistent.
          try {
            await loadServers();
          } catch (serverErr) {
            const message = serverErr instanceof Error ? serverErr.message : 'Failed to load servers from local storage.';
            setStartupInitError(`Startup completed with warnings: ${message}`);
            setStartupErrorDetails((prev) => prev ?? `loadServers error: ${message}`);
          }
          // Clear in-memory identity state regardless of phase — both
          // 'ready' (fully registered) and 'keys_ready' (local keys only)
          // need to be reset so the user starts with IdentitySetup.
          const { phase: currentPhase } = useIdentityStore.getState();
          if (currentPhase === 'ready' || currentPhase === 'keys_ready') {
            useIdentityStore.setState({
              identity: null,
              phase: 'uninitialized',
              permissions: null,
              permissionsStatus: 'idle',
              permissionsPseudonymId: null,
              proofInFlight: false,
              provingStatus: 'idle',
              error: null,
              errorDetails: null,
            });
            setSessionToken(null);
          }
        } else {
          try {
            await loadServers();
          } catch (serverErr) {
            const message = serverErr instanceof Error ? serverErr.message : 'Failed to load servers from local storage.';
            setStartupInitError(message);
            setStartupErrorDetails((prev) => prev ?? `loadServers error: ${message}`);
            useIdentityStore.setState({
              phase: 'error',
              error: 'Annex could not load your saved server list. You can retry startup or clear local state.',
            });
          }
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : 'Failed to initialize local identity state.';
        setStartupInitError(message);
        setStartupErrorDetails((prev) => prev ?? `startup bootstrap error: ${message}`);
        useIdentityStore.setState({
          phase: 'error',
          error: 'Annex failed to start. Try retrying startup or clearing local state.',
          errorDetails: message,
        });
      } finally {
        setIdentityChecked(true);
      }
    };
    void bootstrap();
  }, [loadIdentities, loadServers, inTauri, bootstrapAttempt]);

  const retryBootstrap = () => {
    setIdentityChecked(false);
    setBootstrapAttempt((v) => v + 1);
  };

  return {
    identityChecked,
    startupInitError,
    startupErrorDetails,
    setStartupErrorDetails,
    provingFailures,
    setProvingFailures,
    retryBootstrap,
  };
}
