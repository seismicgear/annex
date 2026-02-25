/**
 * Startup mode selector — shown on every deployment type (Tauri, web, Docker).
 *
 * Lets the user choose between:
 *   - Tauri: "Host a Server" (embedded Axum + cloudflared tunnel) or "Connect to a Server"
 *   - Web/Docker: "Use this server" (current origin) or "Connect to another server"
 *
 * The choice is persisted (Tauri: disk via IPC, Web: localStorage) so
 * subsequent visits skip this screen. Logout clears the preference.
 */

import { useState, useEffect, useCallback } from 'react';
import { isTauri } from '@/lib/tauri';
import { setApiBaseUrl } from '@/lib/api';
import { clearWebStartupMode } from '@/lib/startup-prefs';

const STORAGE_KEY = 'annex:startup-mode';

interface WebPrefs {
  mode: 'local' | 'remote';
  server_url?: string;
}

interface Props {
  onReady: (tunnelUrl?: string) => void;
}

type Phase =
  | 'loading'
  | 'choose'
  | 'starting_server'
  | 'creating_tunnel'
  | 'tunnel_ready'
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
  const [tunnelUrl, setTunnelUrl] = useState('');
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState('');
  const inTauri = isTauri();

  // ── Tauri host mode ──
  const applyHost = useCallback(
    async (skipSave: boolean) => {
      if (!inTauri) return;
      const { startEmbeddedServer, startTunnel, saveStartupMode } = await import('@/lib/tauri');
      setError('');
      try {
        setPhase('starting_server');
        const url = await startEmbeddedServer();
        setApiBaseUrl(url);
        if (!skipSave) {
          await saveStartupMode({ startup_mode: { mode: 'host' } });
        }
        setPhase('creating_tunnel');
        try {
          const pubUrl = await startTunnel();
          setTunnelUrl(pubUrl);
          onReady(pubUrl);
        } catch (tunnelErr) {
          console.warn('Tunnel creation failed:', tunnelErr);
          onReady();
        }
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
        const resp = await fetch(`${normalized}/api/public/server/summary`);
        if (!resp.ok) throw new Error(`Server responded with ${resp.status}`);
      } catch {
        setError('Could not reach server. Check the URL and try again.');
        setPhase('choose');
        return;
      }

      setApiBaseUrl(normalized);
      if (!skipSave) {
        if (inTauri) {
          const { saveStartupMode } = await import('@/lib/tauri');
          await saveStartupMode({
            startup_mode: { mode: 'client', server_url: normalized },
          });
        } else {
          saveWebPrefs({ mode: 'remote', server_url: normalized });
        }
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

  // On mount, check for saved preference
  useEffect(() => {
    let cancelled = false;

    (async () => {
      try {
        if (inTauri) {
          const { getStartupMode } = await import('@/lib/tauri');
          const prefs = await getStartupMode();
          if (cancelled) return;
          if (!prefs) {
            // No saved preference — show the choice screen so the user
            // explicitly picks host vs. client.
            setPhase('choose');
            return;
          }
          if (prefs.startup_mode.mode === 'host') {
            await applyHost(true);
          } else {
            const url = prefs.startup_mode.server_url;
            setRemoteUrl(url);
            await applyRemote(url, true);
          }
        } else {
          // Web/Docker
          const prefs = loadWebPrefs();
          if (cancelled) return;
          if (!prefs) {
            setPhase('choose');
            return;
          }
          if (prefs.mode === 'local') {
            applyLocal(true);
          } else if (prefs.server_url) {
            setRemoteUrl(prefs.server_url);
            await applyRemote(prefs.server_url, true);
          } else {
            setPhase('choose');
          }
        }
      } catch {
        if (!cancelled) setPhase('choose');
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [applyHost, applyRemote, applyLocal, inTauri]);

  const handleReset = async () => {
    if (inTauri) {
      const { clearStartupMode } = await import('@/lib/tauri');
      await clearStartupMode().catch(() => {});
    } else {
      clearWebStartupMode();
    }
    setPhase('choose');
    setError('');
    setTunnelUrl('');
  };

  const handleClientSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    applyRemote(remoteUrl, false);
  };

  const handleCopyUrl = async () => {
    try {
      await navigator.clipboard.writeText(tunnelUrl);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Fallback: select the input text
    }
  };

  // ── Render phases ──

  if (phase === 'loading') {
    return (
      <div className="startup-mode-selector">
        <div className="startup-loading">Loading...</div>
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

  if (phase === 'creating_tunnel') {
    return (
      <div className="startup-mode-selector">
        <h2>Annex</h2>
        <div className="startup-loading">Generating public URL...</div>
        <p className="tunnel-hint">
          This may take a moment on first launch while the tunnel binary is
          downloaded.
        </p>
      </div>
    );
  }

  if (phase === 'tunnel_ready') {
    return (
      <div className="startup-mode-selector">
        <h2>Annex</h2>
        <p className="startup-description">
          Your server is running. Share this URL with others to let them
          connect:
        </p>

        <div className="tunnel-url-card">
          <div className="tunnel-url-display">
            <input
              type="text"
              readOnly
              value={tunnelUrl}
              className="tunnel-url-input"
              onClick={(e) => (e.target as HTMLInputElement).select()}
            />
            <button
              className="tunnel-copy-btn"
              onClick={handleCopyUrl}
            >
              {copied ? 'Copied!' : 'Copy'}
            </button>
          </div>
          <p className="tunnel-url-note">
            This URL is active as long as Annex is running on this device.
          </p>
        </div>

        <button className="primary-btn tunnel-continue-btn" onClick={() => onReady()}>
          Continue to Annex
        </button>
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
        Choose how to use Annex.
      </p>

      <div className="startup-options">
        {inTauri ? (
          /* Tauri: Host a Server */
          <div className="startup-option">
            <h3>Host a Server</h3>
            <p>
              Run your own Annex server on this device. A public URL will be
              generated automatically so others can connect to you.
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
