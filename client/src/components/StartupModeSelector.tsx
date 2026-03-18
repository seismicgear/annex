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
  getLiveKitConfig,
  startLocalLiveKit,
  getStartupMode,
  clearStartupMode,
  acquirePublicEndpoint,
  checkLiveKitReachable,
  desktopCorsGuidance,
} from '@/lib/tauri';
import { setApiBaseUrl, fetchWithTimeout } from '@/lib/api';
import { clearWebStartupMode } from '@/lib/startup-prefs';
import { useServersStore } from '@/stores/servers';
import { useIdentityStore } from '@/stores/identity';

const STORAGE_KEY = 'annex:startup-mode';

interface WebPrefs {
  mode: 'local' | 'remote';
  server_url?: string;
}

/** Describes which optional subsystems failed during host startup. */
export interface DegradedStartupInfo {
  voiceFailed: boolean;
  publicEndpointFailed: boolean;
  /** True when the router was reached but does not proxy LiveKit. */
  livekitRouteUnavailable: boolean;
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

// ── localStorage helpers (web/Docker) ──

function loadWebPrefs(): WebPrefs | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    return raw ? (JSON.parse(raw) as WebPrefs) : null;
  } catch {
    return null;
  }
}

function saveWebPrefs(prefs: WebPrefs): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // Storage full or blocked — non-fatal.
  }
}

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
      const degraded: DegradedStartupInfo = { voiceFailed: false, publicEndpointFailed: false, livekitRouteUnavailable: false };
      try {
        // Auto-configure voice: start a local LiveKit server if not already configured.
        // If configured but unreachable, fall back to starting a local instance.
        // Must happen BEFORE startEmbeddedServer so env vars are picked up.
        setPhase('starting_voice');
        try {
          const lkConfig = await getLiveKitConfig();
          if (!lkConfig.configured) {
            await startLocalLiveKit();
          } else {
            // Verify the configured endpoint is actually reachable
            const reachCheck = await checkLiveKitReachable(lkConfig.url);
            if (!reachCheck.reachable) {
              console.warn('Configured LiveKit unreachable, starting local fallback:', reachCheck.error);
              await startLocalLiveKit();
            }
          }
        } catch (voiceErr) {
          degraded.voiceFailed = true;
          degraded.voiceError = voiceErr instanceof Error ? voiceErr.message : String(voiceErr);
          console.warn('Auto-configure voice failed (voice may be unavailable):', voiceErr);
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
        } catch (endpointErr) {
          degraded.publicEndpointFailed = true;
          degraded.publicEndpointError = endpointErr instanceof Error ? endpointErr.message : String(endpointErr);
          console.warn('Public endpoint acquisition failed (invite links may be unavailable):', endpointErr);
        }

        if (!skipSave) {
          await saveStartupMode({ startup_mode: { mode: 'host' } });
        }
        onReady(degraded.voiceFailed || degraded.publicEndpointFailed || degraded.livekitRouteUnavailable ? degraded : undefined);
      } catch (e) {
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
      let normalized = url.trim();
      if (!/^https?:\/\//i.test(normalized)) {
        normalized = `https://${normalized}`;
      }
      try {
        const parsed = new URL(normalized);
        if (!['http:', 'https:'].includes(parsed.protocol)) {
          setError('Only http and https URLs are supported.');
          return;
        }
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
          setError('Server did not respond in time. Check the URL and verify the server is running.');
        } else if (inTauri && isCorsLikely) {
          setError(desktopCorsGuidance());
        } else {
          setError('Could not reach server. Check the URL and try again.');
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
          saveWebPrefs({ mode: 'remote', server_url: normalized });
        }
      }

      // Resolve server-to-identity: if we have a saved server for this URL
      // with a registered identity, select that identity before proceeding.
      const existing = useServersStore.getState().findServerByBaseUrl(normalized);
      if (existing?.identityId) {
        await useIdentityStore.getState().selectIdentity(existing.identityId);
      }

      onReady();
    },
    [onReady, inTauri],
  );

  // ── Use this server (web/Docker — current origin) ──
  const applyLocal = useCallback(
    (skipSave: boolean) => {
      // Empty base URL = relative paths = current origin
      setApiBaseUrl('');
      if (!skipSave) {
        saveWebPrefs({ mode: 'local' });
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
          // Web/Docker — pre-fill only, no auto-resume
          const prefs = loadWebPrefs();
          if (cancelled) return;
          if (!prefs) {
            setPhase('choose');
            return;
          }
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
  }, [inTauri, applyHost, applyRemote]);

  const handleReset = async () => {
    if (inTauri) {
      await clearStartupMode().catch(() => {});
    } else {
      clearWebStartupMode();
    }
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
        <div className="startup-loading">Loading...</div>
      </div>
    );
  }

  if (phase === 'starting_voice') {
    return (
      <div className="startup-mode-selector">
        <h2>Annex</h2>
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
        <h2>Annex</h2>
        <div className="startup-loading">Starting server...</div>
      </div>
    );
  }

  if (phase === 'acquiring_endpoint') {
    return (
      <div className="startup-mode-selector">
        <h2>Annex</h2>
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
        <h2>Annex</h2>
        <div className="startup-loading">Connecting to server...</div>
      </div>
    );
  }

  if (phase === 'error') {
    return (
      <div className="startup-mode-selector">
        <h2>Annex</h2>
        <div className="error-message">{error}</div>
        <button onClick={handleReset}>Try Again</button>
      </div>
    );
  }

  // phase === 'choose'
  return (
    <div className="startup-mode-selector">
      <h2>Annex</h2>
      <p className="startup-description">
        Choose how to use Annex. Remembered values are shown as suggestions.
      </p>

      <div className="startup-options">
        {inTauri ? (
          /* Tauri: Host a Server */
          <div className="startup-option">
            <h3>Host a Server</h3>
            <p>
              Run your own Annex server on this device. A public URL is
              automatically configured so others can connect to you.
              Voice/video calls work locally. Remote voice is available
              only when the router also proxies LiveKit traffic or a
              separate LiveKit deployment is configured.
            </p>
            <button className="primary-btn" onClick={() => applyHost(false)}>
              Start Hosting
            </button>
          </div>
        ) : (
          /* Web/Docker: Use this server */
          <div className="startup-option">
            <h3>Use This Server</h3>
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
          <h3>Connect to {inTauri ? 'a' : 'Another'} Server</h3>
          <p>Join an existing Annex server as a client.</p>
          <form onSubmit={handleClientSubmit}>
            <input
              type="text"
              value={remoteUrl}
              onChange={(e) => setRemoteUrl(e.target.value)}
              placeholder="annex.example.com"
            />
            {error && <div className="form-error">{error}</div>}
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
