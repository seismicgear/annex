/**
 * Drive the cold-start bootstrap: load identities, run the Tauri
 * startup checks, and load the saved server list without deleting saved data.
 *
 * Also owns the startup-error mirror state (so a phase=error transition
 * can capture diagnostic details for the gate UI) and the proof-in-flight
 * sync poll that clears stale "proof generating" status from the store.
 */

import { useEffect, useState } from 'react';
import { useIdentityStore } from '@/stores/identity';
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

  // Missing browser keys or an onboarding marker do not prove that the
  // embedded server is empty. Only an explicit user reset may erase it.
  useEffect(() => {
    const bootstrap = async () => {
      setStartupInitError(null);
      try {
        await loadIdentities();
        await loadServers();
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
  }, [loadIdentities, loadServers, bootstrapAttempt]);

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
