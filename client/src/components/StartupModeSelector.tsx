/**
 * Startup mode selector — shown when the app is running inside Tauri.
 *
 * Lets the user choose between hosting their own server (embedded Axum)
 * or connecting to an existing remote Annex server as a client.
 *
 * When hosting, a cloudflared tunnel is automatically created to generate
 * a public URL that others can use to connect to the server.
 *
 * The choice is persisted to disk so subsequent launches skip this screen.
 * A "Change Mode" option allows resetting the preference.
 */

import { useState, useEffect, useCallback } from 'react';
import {
  getStartupMode,
  saveStartupMode,
  clearStartupMode,
  startEmbeddedServer,
  startTunnel,
  type StartupPrefs,
} from '@/lib/tauri';
import { setApiBaseUrl } from '@/lib/api';

interface Props {
  onReady: () => void;
}

type Phase =
  | 'loading'
  | 'choose'
  | 'starting_server'
  | 'creating_tunnel'
  | 'tunnel_ready'
  | 'connecting'
  | 'error';

export function StartupModeSelector({ onReady }: Props) {
  const [phase, setPhase] = useState<Phase>('loading');
  const [remoteUrl, setRemoteUrl] = useState('');
  const [tunnelUrl, setTunnelUrl] = useState('');
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState('');

  const applyHost = useCallback(
    async (skipSave: boolean) => {
      setPhase('starting_server');
      setError('');
      try {
        const url = await startEmbeddedServer();
        setApiBaseUrl(url);
        if (!skipSave) {
          await saveStartupMode({ startup_mode: { mode: 'host' } });
        }

        // Now create the tunnel for a public URL
        setPhase('creating_tunnel');
        try {
          const pubUrl = await startTunnel();
          setTunnelUrl(pubUrl);
          setPhase('tunnel_ready');
        } catch (tunnelErr) {
          // Tunnel failure is non-fatal — the server still works locally.
          // Proceed to the app but show the error briefly.
          console.warn('Tunnel creation failed:', tunnelErr);
          onReady();
        }
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e));
        setPhase('error');
      }
    },
    [onReady],
  );

  const applyClient = useCallback(
    async (url: string, skipSave: boolean) => {
      setError('');
      let normalized = url.trim();
      if (!/^https?:\/\//i.test(normalized)) {
        normalized = `https://${normalized}`;
      }

      // Validate URL format
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

      // Probe server to verify it's reachable
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
        await saveStartupMode({
          startup_mode: { mode: 'client', server_url: normalized },
        });
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
        const prefs: StartupPrefs | null = await getStartupMode();
        if (cancelled) return;

        if (!prefs) {
          setPhase('choose');
          return;
        }

        if (prefs.startup_mode.mode === 'host') {
          await applyHost(true);
        } else {
          const url = prefs.startup_mode.server_url;
          setRemoteUrl(url);
          await applyClient(url, true);
          // If applyClient failed (set phase to 'choose'), that's fine —
          // user will see the choice screen with the pre-filled URL.
        }
      } catch {
        if (!cancelled) setPhase('choose');
      }
    })();

    return () => {
      cancelled = true;
    };
  }, [applyHost, applyClient]);

  const handleReset = async () => {
    await clearStartupMode().catch(() => {});
    setPhase('choose');
    setError('');
    setTunnelUrl('');
  };

  const handleClientSubmit = (e: React.FormEvent) => {
    e.preventDefault();
    applyClient(remoteUrl, false);
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

        <button className="primary-btn tunnel-continue-btn" onClick={onReady}>
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
        Choose how to use Annex on this device.
      </p>

      <div className="startup-options">
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

        <div className="startup-divider">
          <span>or</span>
        </div>

        <div className="startup-option">
          <h3>Connect to a Server</h3>
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
