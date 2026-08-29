/**
 * Connect the WebSocket, refresh expired session tokens on cold start, and
 * keep the token auto-refresh interval alive while the user is registered.
 *
 * Fires only when the identity reaches `phase === 'ready'` with a known
 * pseudonym; teardown disconnects the socket and stops token refresh.
 */

import { useEffect } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import {
  getApiBaseUrl,
  getSessionToken,
  isTokenExpired,
  refreshSessionToken,
  setSessionToken,
  startTokenRefresh,
  stopTokenRefresh,
} from '@/lib/api';
import { saveIdentity } from '@/lib/db';
import type { IdentityPhase } from '@/stores/identity';

interface UseSessionConnectionArgs {
  phase: IdentityPhase;
  pseudonymId: string | null | undefined;
  connectWs: (pseudonymId: string, baseUrl?: string, sessionToken?: string | null) => void;
  disconnectWs: () => void;
  loadPermissions: () => Promise<void>;
  fetchServerImage: () => Promise<void>;
}

export function useSessionConnection({
  phase,
  pseudonymId,
  connectWs,
  disconnectWs,
  loadPermissions,
  fetchServerImage,
}: UseSessionConnectionArgs): void {
  useEffect(() => {
    if (phase === 'ready' && pseudonymId) {
      let cancelled = false;

      (async () => {
        // Refresh expired tokens before making any API calls
        const currentToken = getSessionToken();
        if (currentToken && isTokenExpired(currentToken)) {
          try {
            const newToken = await refreshSessionToken();
            if (cancelled) return;
            // Persist refreshed token to IndexedDB
            const currentIdentity = useIdentityStore.getState().identity;
            if (currentIdentity) {
              const updated = { ...currentIdentity, sessionToken: newToken };
              await saveIdentity(updated);
              useIdentityStore.setState({ identity: updated });
            }
          } catch (err) {
            if (cancelled) return;
            console.error('session token refresh failed on startup', err);
            // Token refresh failed — session is invalid.
            // Clear the stale in-memory token and fall back to re-registration.
            setSessionToken(null);
            useIdentityStore.setState({ phase: 'keys_ready' });
            return;
          }
        }

        if (cancelled) return;
        const baseUrl = getApiBaseUrl();
        const sessionToken = getSessionToken();
        connectWs(pseudonymId, baseUrl || undefined, sessionToken);
        loadPermissions();
        fetchServerImage();

        // Auto-refresh session token at 80% of 1-hour TTL (~48 min).
        // Persist each refreshed token to IndexedDB so cold starts work.
        // Also update the active WebSocket so reconnects use the fresh token.
        startTokenRefresh(
          3600,
          async (newToken) => {
            const cur = useIdentityStore.getState().identity;
            if (cur) {
              const updated = { ...cur, sessionToken: newToken };
              await saveIdentity(updated);
              useIdentityStore.setState({ identity: updated });
            }
            // Propagate to active WebSocket so reconnects use the refreshed token
            useChannelsStore.getState().updateWsSessionToken(newToken);
          },
          async (err) => {
            // Reached only after the in-window retries are exhausted, so the
            // credential is genuinely gone rather than one request having
            // failed. Take the same route as the cold-start failure above:
            // drop to `keys_ready` so the user is offered re-registration.
            // The alternative is what used to happen — a console line, and
            // an app that looks signed in while every call 401s.
            console.error('session token refresh failed', err);
            setSessionToken(null);
            useIdentityStore.setState({ phase: 'keys_ready' });
          },
        );
      })();

      return () => {
        cancelled = true;
        disconnectWs();
        stopTokenRefresh();
      };
    }
  }, [phase, pseudonymId, connectWs, disconnectWs, loadPermissions, fetchServerImage]);
}
