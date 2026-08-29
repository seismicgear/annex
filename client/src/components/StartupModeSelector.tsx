/**
 * Startup mode selector — shown on every deployment type (Tauri, web, Docker).
 *
 * Lets the user choose between:
 *   - Tauri: "Host a Server" (embedded Axum) or "Connect to a Server"
 *   - Web/Docker: "Use this server" (current origin) or "Connect to another server"
 *
 * The choice is persisted (Tauri: disk via IPC, Web: localStorage) so
 * subsequent visits can be pre-filled with suggestions. Logout clears
 * the preference.
 */

import { useState, useEffect, useCallback } from 'react';
import {
  isTauri,
  startEmbeddedServer,
  saveStartupMode,
  getWebRtcConfig,
  startLocalWebRtc,
  clearWebRtcEnv,
  getStartupMode,
  clearStartupMode,
  acquirePublicEndpoint,
  getPublicEndpoint,
  checkWebRtcReachable,
  desktopCorsGuidance,
} from '@/lib/tauri';
import { setApiBaseUrl, fetchWithTimeout } from '@/lib/api';
import { clearWebStartupMode, loadWebStartupMode, saveWebStartupMode } from '@/lib/startup-prefs';
import { normalizeServerUrl } from '@/lib/url';
import { useServersStore } from '@/stores/servers';
import { useIdentityStore } from '@/stores/identity';
import { useVoiceStore } from '@/stores/voice';


/** Describes which optional subsystems failed during host startup. */
export interface DegradedStartupInfo {
  voiceFailed: boolean;
  publicEndpointFailed: boolean;
  /** True when the router was reached but does not proxy WebRTC. */
  webrtcRouteUnavailable: boolean;
  voiceError?: string;
  publicEndpointError?: string;
}

interface Props {
  onReady: (degraded?: DegradedStartupInfo) => void;
}

type Phase =
  | 'loading'
  | 'choose'
  | 'starting_voice'
  | 'starting_server'
  | 'acquiring_endpoint'
  | 'connecting'
  | 'error';

export function StartupModeSelector({ onReady }: Props) {
  const [phase, setPhase] = useState<Phase>('loading');
  const [remoteUrl, setRemoteUrl] = useState('');
  const [error, setError] = useState('');
  const inTauri = isTauri();

  // ── Tauri host mode ──
  const applyHost = useCallback(
    async (skipSave: boolean) => {
      if (!inTauri) return;
      setError('');
      const degraded: DegradedStartupInfo = { voiceFailed: false, publicEndpointFailed: false, webrtcRouteUnavailable: false };
      try {
        // Clear any stale voice-disabled state from a previous failed attempt
        // so a successful startup does not inherit old failure flags.
        useVoiceStore.getState().setVoiceSessionDisabled(false);

        // Auto-configure voice: start a local WebRTC server if not already configured.
        // If configured but unreachable, fall back to starting a local instance.
        // Must happen BEFORE startEmbeddedServer so env vars are picked up.
        setPhase('starting_voice');
        try {
          const lkConfig = await getWebRtcConfig();
          if (!lkConfig.configured) {
            await startLocalWebRtc();
          } else {
            // Verify the configured endpoint is actually reachable
            const reachCheck = await checkWebRtcReachable(lkConfig.url);
            if (!reachCheck.reachable) {
              console.warn('Configured WebRTC unreachable, starting local fallback:', reachCheck.error);
              await startLocalWebRtc();
            }
          }
        } catch (voiceErr) {
          degraded.voiceFailed = true;
          degraded.voiceError = voiceErr instanceof Error ? voiceErr.message : String(voiceErr);
          console.warn('Auto-configure voice failed (voice may be unavailable):', voiceErr);
          // Clear WebRTC env vars so the embedded server does not pick up
          // the dev fallback URL when WebRTC actually failed to start.
          try { await clearWebRtcEnv(); } catch { /* best effort */ }
          // Propagate disabled state to the voice store so VoicePanel knows
          // not to offer Join/Create Call with stale fallback config.
          useVoiceStore.getState().setVoiceSessionDisabled(
            true,
            `Voice unavailable: ${degraded.voiceError}`,
          );
        }

        setPhase('starting_server');
        const url = await startEmbeddedServer();
        setApiBaseUrl(url);

        // Acquire a public endpoint via the Annex router so the server is
        // reachable from the internet. The returned URL is used as the
        // server's public URL for invite links.
        setPhase('acquiring_endpoint');
        try {
          await acquirePublicEndpoint();
          // Check whether the router proxies WebRTC traffic
          try {
            const epInfo = await getPublicEndpoint();
            if (epInfo && !epInfo.public_webrtc_url) {
              degraded.webrtcRouteUnavailable = true;
            }
          } catch {
            // Non-fatal — the endpoint query may not be available yet
          }
        } catch (endpointErr) {
          degraded.publicEndpointFailed = true;
          degraded.publicEndpointError = endpointErr instanceof Error ? endpointErr.message : String(endpointErr);
          console.warn('Public endpoint acquisition failed (invite links may be unavailable):', endpointErr);
        }

        if (!skipSave) {
          await saveStartupMode({ startup_mode: { mode: 'host' } });
        }
        onReady(degraded.voiceFailed || degraded.publicEndpointFailed || degraded.webrtcRouteUnavailable ? degraded : undefined);
      } catch (e) {
        if (inTauri) {
          await clearStartupMode().catch(() => {});
        } else {
          clearWebStartupMode();
        }
        setError(e instanceof Error ? e.message : String(e));
        setPhase('error');
      }
    },
    [onReady, inTauri],
  );

  // ── Connect to a remote server (shared by Tauri + web) ──
  const applyRemote = useCallback(
    async (url: string, skipSave: boolean) => {
      setError('');
      let normalized: string;
      try {
        normalized = normalizeServerUrl(url);
      } catch {
        setError('Invalid URL format.');
        return;
      }

      setPhase('connecting');

      try {
        const resp = await fetchWithTimeout(`${normalized}/api/public/server/summary`, undefined, 15_000);
        if (!resp.ok) throw new Error(`Server responded with ${resp.status}`);
      } catch (err) {
        // Distinguish CORS / network / timeout errors for better diagnostics
        const isTimeout = err instanceof Error && /timed out/i.test(err.message);
        const isCorsLikely = err instanceof TypeError && /failed to fetch/i.test(err.message);
        if (isTimeout) {
          setError(`Server at ${normalized} did not respond in time. Check the URL and verify the server is running.`);
        } else if (inTauri && isCorsLikely) {
          setError(desktopCorsGuidance());
        } else {
          setError(`Could not reach server at ${normalized}. Check the URL and try again.`);
        }
        setPhase('choose');
        return;
      }

      setApiBaseUrl(normalized);
      if (!skipSave) {
        if (inTauri) {
          await saveStartupMode({
            startup_mode: { mode: 'client', server_url: normalized },
          });
        } else {
          saveWebStartupMode({ mode: 'remote', server_url: normalized });
        }
      }

      // Clear any stale voice-disabled state from a previous host-mode failure
      // so connecting to a remote server does not inherit the disabled flag.
      useVoiceStore.getState().setVoiceSessionDisabled(false);

      // Resolve server-to-identity: if we have a saved server for this URL
      // with a registered identity, select that identity before proceeding.
      const existing = useServersStore.getState().findServerByBaseUrl(normalized);
      if (existing?.identityId) {
        await useIdentityStore.getState().selectIdentity(existing.identityId);
        onReady();
      } else {
        // No existing identity for this server — route through remote
        // registration so App.tsx Gate 3 creates a proper identity/server pair.
        const server = await useServersStore.getState().beginRemoteRegistration(normalized);
        if (!server) {
          setError('Failed to begin registration with the remote server.');
          setPhase('choose');
          return;
        }
        onReady();
      }
    },
    [onReady, inTauri],
  );

  // ── Use this server (web/Docker — current origin) ──
  const applyLocal = useCallback(
    (skipSave: boolean) => {
      // Empty base URL = relative paths = current origin
      setApiBaseUrl('');
      // Clear any stale voice-disabled state from a previous startup failure.
      useVoiceStore.getState().setVoiceSessionDisabled(false);
      if (!skipSave) {
        saveWebStartupMode({ mode: 'local' });
      }
      onReady();
    },
    [onReady],
  );

  // On mount, load saved preference. For returning Tauri host users,
  // auto-resume (skip the selector entirely). For Tauri client and
  // web/Docker modes, pre-fill form state for manual selection.
  useEffect(() => {
    let cancelled = false;

    (async () => {
      try {
        if (inTauri) {
          const prefs = await getStartupMode();
          if (cancelled) return;
          if (!prefs) {
            setPhase('choose');
            return;
          }
          // Auto-resume: apply saved mode without re-saving (skipSave=true)
          if (prefs.startup_mode.mode === 'host') {
            void applyHost(true);
            return;
          }
          if (prefs.startup_mode.mode === 'client' && prefs.startup_mode.server_url) {
            // Pre-fill the URL as a suggestion — let the user decide to connect.
            // Auto-resuming a remote server that may be unreachable gives poor UX
            // (brief "Connecting..." flash then an error with no pre-filled URL).
            setRemoteUrl(prefs.startup_mode.server_url);
            setPhase('choose');
            return;
          }
          setPhase('choose');
        } else {
          const prefs = loadWebStartupMode();
          if (cancelled) return;
          if (!prefs) {
            setPhase('choose');
            return;
          }
          // Auto-resume "use this server". The objection to auto-resuming a
          // remote URL — a "Connecting..." flash followed by an error if the
          // host is unreachable — does not apply here: `local` means the
          // current origin, which is by definition reachable because it just
          // served this page. Without this, a returning web user was asked to
          // re-pick a server they were already registered with on every single
          // page load, and answering that prompt drove a redundant
          // re-registration.
          if (prefs.mode === 'local') {
            applyLocal(true);
            return;
          }
          // Remote stays pre-fill-only, so an unreachable saved host does not
          // strand the user on an error screen with an empty URL field.
          if (prefs.mode === 'remote' && prefs.server_url) {
            setRemoteUrl(prefs.server_url);
          }
          setPhase('choose');
        }
      } catch {
        if (!cancelled) setPhase('choose');
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [inTauri, applyHost, applyRemote, applyLocal]);

  const handleReset = async () => {
    if (inTauri) {
      await clearStartupMode().catch(() => {});
    } else {
      clearWebStartupMode();
    }
    // Clear stale voice-disabled state so returning to the chooser
    // does not preserve a previous host-start failure.
    useVoiceStore.getState().setVoiceSessionDisabled(false);
    setPhase('choose');
    setError('');
  };

  const handleClientSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    applyRemote(remoteUrl, false);
  };

  // ── Render phases ──

  if (phase === 'loading') {
    return (
      <div className="startup-mode-selector">
        {/* Every other phase of this screen renders the `h1`; this one used to
            drop it, so while startup preferences were being read the page had
            no level-one heading at all. axe's `page-has-heading-one` would
            have caught it, but no audit surface reached this phase. */}
        <h1>Annex</h1>
        <div className="startup-loading">Loading...</div>
      </div>
    );
  }

  if (phase === 'starting_voice') {
    return (
      <div className="startup-mode-selector">
        <h1>Annex</h1>
        <div className="startup-loading">Setting up voice...</div>
        <p className="tunnel-hint">
          This may take a moment on first launch while the voice server is
          downloaded.
        </p>
      </div>
    );
  }

  if (phase === 'starting_server') {
    return (
      <div className="startup-mode-selector">
        <h1>Annex</h1>
        <div className="startup-loading">Starting server...</div>
      </div>
    );
  }

  if (phase === 'acquiring_endpoint') {
    return (
      <div className="startup-mode-selector">
        <h1>Annex</h1>
        <div className="startup-loading">Setting up public access...</div>
        <p className="endpoint-hint">
          Acquiring a public endpoint from the Annex router so others can
          connect to your server.
        </p>
      </div>
    );
  }

  if (phase === 'connecting') {
    return (
      <div className="startup-mode-selector">
        <h1>Annex</h1>
        <div className="startup-loading">Connecting to server...</div>
      </div>
    );
  }

  if (phase === 'error') {
    return (
      <div className="startup-mode-selector">
        <h1>Annex</h1>
        <div className="error-message" role="alert">{error}</div>
        <button onClick={handleReset}>Try Again</button>
      </div>
    );
  }

  // phase === 'choose'
  return (
    <div className="startup-mode-selector">
      <h1>Annex</h1>
      <p className="startup-description">
        Choose how to use Annex. Remembered values are shown as suggestions.
      </p>

      <div className="startup-options">
        {inTauri ? (
          /* Tauri: Host a Server */
          <div className="startup-option">
            <h2>Host a Server</h2>
            <p>
              Run your own Annex server on this device. A public URL is
              automatically configured so others can connect to you.
              Voice/video calls work locally. Remote voice is available
              only when the router also proxies WebRTC traffic or a
              separate WebRTC deployment is configured.
            </p>
            <button className="primary-btn" onClick={() => applyHost(false)}>
              Start Hosting
            </button>
          </div>
        ) : (
          /* Web/Docker: Use this server */
          <div className="startup-option">
            <h2>Use This Server</h2>
            <p>
              Connect to the Annex server at the current address.
            </p>
            <button className="primary-btn" onClick={() => applyLocal(false)}>
              Continue
            </button>
          </div>
        )}

        <div className="startup-divider">
          <span>or</span>
        </div>

        <div className="startup-option">
          <h2>Connect to {inTauri ? 'a' : 'Another'} Server</h2>
          <p>Join an existing Annex server as a client.</p>
          <form onSubmit={handleClientSubmit}>
            <input
              type="text"
              value={remoteUrl}
              onChange={(e) => setRemoteUrl(e.target.value)}
              placeholder="annex.example.com"
            />
            {error && <div className="form-error" role="alert">{error}</div>}
            <button
              type="submit"
              className="primary-btn"
              disabled={!remoteUrl.trim()}
            >
              Connect
            </button>
          </form>
        </div>
      </div>
    </div>
  );
}
