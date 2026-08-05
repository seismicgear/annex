/**
 * Drive everything that happens between "user picked a server" and "fully
 * registered + persisted":
 *
 *   • Auto-register the local identity once the chosen server is ready.
 *   • Reset to the server selector when the user logs out.
 *   • Persist the server to the node hub on first successful registration.
 *   • In Tauri host mode, publish the router-issued public URL (and
 *     WebRTC public URL) back to the running server.
 *
 * Cross-cutting state (serverReady, password, pending invite codes, etc.)
 * is owned by AppShell so it can be read by the gate UI; this hook just
 * drives the effects.
 */

import { useEffect, useRef, type Dispatch, type MutableRefObject, type SetStateAction } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useServersStore } from '@/stores/servers';
import {
  getApiBaseUrl,
  getServerSummary,
  setPublicUrl,
  setWebrtcPublicUrl,
} from '@/lib/api';
import { createInviteLink } from '@/lib/invite';
import { getPersonasForIdentity } from '@/lib/personas';
import { clearWebStartupMode } from '@/lib/startup-prefs';
import {
  clearStartupMode as clearTauriStartupMode,
  getPublicEndpoint,
  isTauri,
  markFirstRunCompleted,
} from '@/lib/tauri';
import { ApiError } from '@/lib/api';
import type { DegradedStartupInfo } from '@/components/StartupModeSelector';
import type { IdentityPhase } from '@/stores/identity';
import type { StoredIdentity } from '@/types';

/**
 * Did the server answer and refuse us, as opposed to being unreachable?
 *
 * A refusal is a decision — wrong password, bad invite, server full, not
 * permitted — and will be made identically however many times we ask. Only a
 * transport-level failure is worth retrying.
 *
 * 429 is deliberately excluded: being rate limited means we have already asked
 * too often, so hammering it further is exactly the wrong response.
 */
function isRefusal(err: unknown): boolean {
  return (
    err instanceof ApiError &&
    [400, 401, 403, 409, 413, 429].includes(err.status)
  );
}

interface UseServerSelectionArgs {
  phase: IdentityPhase;
  identity: StoredIdentity | null;
  serverReady: boolean;
  inTauri: boolean;
  registerWithServer: (serverSlug: string, inviteCode?: string, serverPassword?: string) => Promise<void>;
  saveCurrentServer: (identityId: string, slug: string, label: string, baseUrl?: string) => Promise<void>;
  pendingInviteCode: string | null;
  pendingServerSlug: string | null;
  serverPassword: string;
  startupFlowId: MutableRefObject<number>;
  setServerReady: Dispatch<SetStateAction<boolean>>;
  setPasswordRequired: Dispatch<SetStateAction<boolean>>;
  setServerPassword: Dispatch<SetStateAction<string>>;
  setPendingInviteCode: Dispatch<SetStateAction<string | null>>;
  setPendingServerSlug: Dispatch<SetStateAction<string | null>>;
  setStartupErrorDetails: Dispatch<SetStateAction<string | null>>;
  setDegradedStartup: Dispatch<SetStateAction<DegradedStartupInfo | null>>;
}

export function useServerSelection({
  phase,
  identity,
  serverReady,
  inTauri,
  registerWithServer,
  saveCurrentServer,
  pendingInviteCode,
  pendingServerSlug,
  serverPassword,
  startupFlowId,
  setServerReady,
  setPasswordRequired,
  setServerPassword,
  setPendingInviteCode,
  setPendingServerSlug,
  setStartupErrorDetails,
  setDegradedStartup,
}: UseServerSelectionArgs): void {
  /** Track which identity+server pairs have already been saved to avoid duplicates. */
  const savedServerKeys = useRef(new Set<string>());
  const prevPhaseRef = useRef(phase);

  // ── Register identity with server after user selects a server ──
  // Only fires when phase is exactly 'keys_ready' (keys exist, not yet
  // registered) and the user has explicitly picked a server on Screen 2.
  //
  // Retries getServerSummary() with exponential backoff because the
  // server may still be initialising when serverReady flips to true.
  useEffect(() => {
    if (!serverReady || phase !== 'keys_ready' || !identity?.sk) return;
    let cancelled = false;
    const flowId = startupFlowId.current;
    const isCurrentFlow = () => startupFlowId.current === flowId;

    const MAX_RETRIES = 5;
    const BASE_DELAY_MS = 500;
    const apiBaseUrl = getApiBaseUrl() || 'same-origin';
    const origin = window.location.origin;

    (async () => {
      let lastError: unknown;
      for (let attempt = 0; attempt <= MAX_RETRIES; attempt++) {
        if (cancelled) return;
        try {
          const summary = await getServerSummary();
          if (cancelled || !isCurrentFlow()) return;
          const slug = pendingServerSlug ?? summary.slug;

          // If the server requires a password and we don't have one yet,
          // show a password prompt and wait for the user to provide it.
          if (summary.access_mode === 'password' && !serverPassword) {
            if (!isCurrentFlow()) return;
            setPasswordRequired(true);
            return; // Effect will re-run when serverPassword changes
          }

          if (!cancelled && isCurrentFlow()) {
            await registerWithServer(slug, pendingInviteCode ?? undefined, serverPassword || undefined);
            if (!isCurrentFlow()) return;
            setPendingInviteCode(null);
            setPendingServerSlug(null);
            setPasswordRequired(false);
            setServerPassword('');
          }
          return;
        } catch (err) {
          lastError = err;
          console.error('startup_registration_retry_failed', {
            apiBaseUrl,
            origin,
            attempt,
            maxRetries: MAX_RETRIES,
            error: err,
          });

          // Retrying only makes sense when the server could not be reached.
          // A server that answered and REFUSED us — wrong password, invalid
          // invite, server full, not allowed — will refuse identically five
          // more times. Retrying anyway burned six of the ten-per-minute
          // registration budget on a single wrong password, so a second
          // attempt hit the rate limiter instead of the login.
          if (isRefusal(err)) break;

          if (attempt < MAX_RETRIES) {
            await new Promise((r) => setTimeout(r, BASE_DELAY_MS * 2 ** attempt));
          }
        }
      }
      if (!cancelled && isCurrentFlow()) {
        const message = lastError instanceof Error ? lastError.message : 'Failed to reach server';
        const likelyNetworkError =
          lastError instanceof TypeError && /failed to fetch/i.test(lastError.message);
        const networkHint = inTauri && likelyNetworkError
          ? ' Desktop mode hint: this may be a CORS/origin mismatch between the embedded app origin and API base URL.'
          : '';
        const safeDiagnostic = `Unable to contact server (API: ${apiBaseUrl}, app origin: ${origin}).${networkHint}`;

        setStartupErrorDetails(
          [
            `apiBaseUrl: ${apiBaseUrl}`,
            `origin: ${origin}`,
            `attempts: ${MAX_RETRIES + 1}`,
            `error: ${message}`,
          ].join('\n'),
        );
        useIdentityStore.setState({
          phase: 'error',
          error: safeDiagnostic,
        });

        // Clean up the placeholder server entry so the hub does not strand
        // a permanently disabled icon after a failed registration.
        useServersStore.getState().cleanupFailedRegistration();
      }
    })();
    return () => { cancelled = true; };
  }, [
    serverReady,
    phase,
    identity?.sk,
    registerWithServer,
    inTauri,
    pendingInviteCode,
    pendingServerSlug,
    serverPassword,
    startupFlowId,
    setPasswordRequired,
    setPendingInviteCode,
    setPendingServerSlug,
    setServerPassword,
    setStartupErrorDetails,
  ]);

  // When the user logs out, return to the mode selector.
  // We track the previous phase so we only reset when phase *transitions*
  // to 'uninitialized' (a real logout), not when it was already 'uninitialized'.
  useEffect(() => {
    const prevPhase = prevPhaseRef.current;
    prevPhaseRef.current = phase;

    if (phase === 'uninitialized' && prevPhase !== 'uninitialized' && serverReady) {
      // Clear startup preferences for the appropriate platform.
      // Desktop (Tauri) uses disk-based startup_prefs.json; web uses localStorage.
      if (isTauri()) {
        clearTauriStartupMode().catch(() => {});
      }
      clearWebStartupMode();
      setServerReady(false);
      savedServerKeys.current.clear();
    }
  }, [phase, serverReady, setServerReady]);

  // Auto-set public endpoint as server public URL and create invite link (Tauri host mode)
  useEffect(() => {
    if (!inTauri || phase !== 'ready' || !identity?.pseudonymId) return;
    let cancelled = false;
    (async () => {
      try {
        const endpointInfo = await getPublicEndpoint();
        if (cancelled || !endpointInfo) return;
        // Set the router-provided URL as the server's public URL
        await setPublicUrl(identity.pseudonymId!, endpointInfo.public_url);
        // Push the WebRTC public URL into the running server so remote
        // voice join responses return a globally-reachable URL.
        if (endpointInfo.public_webrtc_url) {
          await setWebrtcPublicUrl(identity.pseudonymId!, endpointInfo.public_webrtc_url).catch(() => {
            // Non-fatal: remote voice may use fallback URL
          });
        } else {
          // No WebRTC URL from router — remote voice/video will not work.
          setDegradedStartup((prev) => ({
            voiceFailed: prev?.voiceFailed ?? false,
            publicEndpointFailed: prev?.publicEndpointFailed ?? false,
            webrtcRouteUnavailable: true,
            voiceError: prev?.voiceError,
            publicEndpointError: prev?.publicEndpointError,
          }));
        }
        // Pre-create an invite link so it's ready when the user visits settings
        await createInviteLink(getApiBaseUrl(), identity.pseudonymId!).catch(() => {});
      } catch {
        // Non-fatal — invite links may be unavailable without a public URL
      }
    })();
    return () => { cancelled = true; };
  }, [inTauri, phase, identity?.pseudonymId, setDegradedStartup]);

  // Auto-save current server to the node hub on every successful registration.
  // Keyed by identity+baseUrl so it runs for 2nd, 3rd servers (not just first).
  // Also fulfills the placeholder entry if beginRemoteRegistration was used.
  useEffect(() => {
    if (phase !== 'ready' || !identity?.pseudonymId || !identity.id) return;

    const activeBaseUrl = getApiBaseUrl();
    const saveKey = `${identity.id}:${activeBaseUrl}`;
    if (savedServerKeys.current.has(saveKey)) return;
    savedServerKeys.current.add(saveKey);

    const pendingServerId = useServersStore.getState().pendingRegistrationServerId;

    (async () => {
      try {
        // If a placeholder was being tracked, fulfill it directly
        if (pendingServerId) {
          await useServersStore.getState().fulfillPlaceholder(
            pendingServerId,
            identity.id,
            identity.serverSlug,
          );
        } else {
          await saveCurrentServer(identity.id, identity.serverSlug, identity.serverSlug, activeBaseUrl);
        }

        // Mark first-run as completed so subsequent launches skip cleanup
        if (inTauri) markFirstRunCompleted().catch(() => {});

        const personas = await getPersonasForIdentity(identity.id);
        if (personas.length > 0) {
          const server = useServersStore.getState().getActiveServer();
          if (server && !server.personaId) {
            useServersStore.getState().setServerPersona(
              server.id,
              personas[0].id,
              personas[0].accentColor,
            );
          }
        }
      } catch {
        // Non-fatal: server hub entry may not be saved
        // Remove key so it can be retried
        savedServerKeys.current.delete(saveKey);
      }
    })();
  }, [phase, identity?.pseudonymId, identity?.id, identity?.serverSlug, saveCurrentServer, inTauri]);
}
