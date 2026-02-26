/**
 * LiveKit settings panel for desktop (Tauri) mode.
 *
 * Allows users to configure LiveKit credentials, check connectivity,
 * and optionally start a local LiveKit server for host mode.
 */

import { useEffect, useState, useCallback } from 'react';
import {
  isTauri,
  getLiveKitConfig,
  saveLiveKitConfig,
  clearLiveKitConfig,
  checkLiveKitReachable,
  startLocalLiveKit,
  stopLocalLiveKit,
  getLocalLiveKitUrl,
} from '@/lib/tauri';

export function LiveKitSettings() {
  const [url, setUrl] = useState('');
  const [apiKey, setApiKey] = useState('');
  const [apiSecret, setApiSecret] = useState('');
  const [tokenTtl, setTokenTtl] = useState(3600);
  const [configured, setConfigured] = useState(false);
  const [hasSecret, setHasSecret] = useState(false);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState<string | null>(null);
  const [reachable, setReachable] = useState<boolean | null>(null);
  const [checking, setChecking] = useState(false);
  const [localLiveKitUrl, setLocalLiveKitUrl] = useState<string | null>(null);
  const [startingLocal, setStartingLocal] = useState(false);

  const loadConfig = useCallback(async () => {
    if (!isTauri()) return;
    try {
      const config = await getLiveKitConfig();
      setUrl(config.url);
      setApiKey(config.api_key);
      setConfigured(config.configured);
      setHasSecret(config.has_api_secret);
      setTokenTtl(config.token_ttl_seconds);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadConfig();
    // Check if local LiveKit is running
    if (isTauri()) {
      getLocalLiveKitUrl().then(setLocalLiveKitUrl).catch(() => {});
    }
  }, [loadConfig]);

  const handleSave = async () => {
    setSaving(true);
    setError(null);
    setSuccess(null);
    try {
      await saveLiveKitConfig({
        url,
        api_key: apiKey,
        api_secret: apiSecret,
        token_ttl_seconds: tokenTtl,
      });
      setSuccess('LiveKit configuration saved. Restart the application for changes to take effect.');
      setApiSecret(''); // Clear from UI after save
      await loadConfig();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const handleClear = async () => {
    setError(null);
    setSuccess(null);
    try {
      await clearLiveKitConfig();
      setUrl('');
      setApiKey('');
      setApiSecret('');
      setTokenTtl(3600);
      setSuccess('LiveKit configuration cleared.');
      await loadConfig();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const handleCheckReachable = async () => {
    if (!url) return;
    setChecking(true);
    setReachable(null);
    try {
      const result = await checkLiveKitReachable(url);
      setReachable(result.reachable);
      if (!result.reachable && result.error) {
        setError(`LiveKit not reachable: ${result.error}`);
      }
    } catch (err) {
      setReachable(false);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setChecking(false);
    }
  };

  const handleStartLocal = async () => {
    setStartingLocal(true);
    setError(null);
    try {
      const result = await startLocalLiveKit();
      setLocalLiveKitUrl(result.url);
      setUrl(result.url);
      setSuccess('Local LiveKit server started. Voice will be available when the embedded server starts.');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setStartingLocal(false);
    }
  };

  const handleStopLocal = async () => {
    try {
      await stopLocalLiveKit();
      setLocalLiveKitUrl(null);
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  if (!isTauri()) return null;
  if (loading) return <p className="admin-loading">Loading LiveKit settings...</p>;

  return (
    <div className="admin-section">
      <h3>Voice (LiveKit)</h3>

      <div className="admin-status-row">
        <span>Status: </span>
        <strong style={{ color: configured ? '#4caf50' : '#888' }}>
          {configured ? 'Configured' : 'Not configured'}
        </strong>
        {hasSecret && <span style={{ color: '#666', marginLeft: '0.5rem' }}>(secret stored)</span>}
      </div>

      {localLiveKitUrl && (
        <div className="admin-status-row" style={{ marginTop: '0.5rem' }}>
          <span>Local LiveKit: </span>
          <strong style={{ color: '#4caf50' }}>{localLiveKitUrl}</strong>
          <button onClick={handleStopLocal} className="admin-btn-sm" style={{ marginLeft: '0.5rem' }}>
            Stop
          </button>
        </div>
      )}

      {!localLiveKitUrl && (
        <div style={{ marginTop: '0.5rem' }}>
          <button
            onClick={handleStartLocal}
            disabled={startingLocal}
            className="admin-btn"
          >
            {startingLocal ? 'Starting...' : 'Start Local LiveKit Server'}
          </button>
          <span className="admin-hint" style={{ marginLeft: '0.5rem' }}>
            Downloads and runs a local LiveKit server for voice in host mode.
          </span>
        </div>
      )}

      <div className="admin-form" style={{ marginTop: '1rem' }}>
        <label>
          LiveKit URL
          <input
            type="text"
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="ws://localhost:7880"
          />
        </label>

        <label>
          API Key
          <input
            type="text"
            value={apiKey}
            onChange={(e) => setApiKey(e.target.value)}
            placeholder="API key"
          />
        </label>

        <label>
          API Secret
          <input
            type="password"
            value={apiSecret}
            onChange={(e) => setApiSecret(e.target.value)}
            placeholder={hasSecret ? '(stored securely)' : 'API secret'}
          />
        </label>

        <label>
          Token TTL (seconds)
          <input
            type="number"
            value={tokenTtl}
            onChange={(e) => setTokenTtl(parseInt(e.target.value, 10) || 3600)}
            min={60}
            max={86400}
          />
        </label>

        <div className="admin-btn-row">
          <button onClick={handleSave} disabled={saving || !url} className="admin-btn">
            {saving ? 'Saving...' : 'Save'}
          </button>
          <button onClick={handleCheckReachable} disabled={checking || !url} className="admin-btn-secondary">
            {checking ? 'Checking...' : 'Test Connection'}
          </button>
          {configured && (
            <button onClick={handleClear} className="admin-btn-danger">
              Clear
            </button>
          )}
        </div>

        {reachable !== null && (
          <p style={{ color: reachable ? '#4caf50' : '#e63946', marginTop: '0.5rem' }}>
            {reachable ? 'LiveKit server is reachable.' : 'LiveKit server is not reachable.'}
          </p>
        )}
      </div>

      {error && <p className="admin-error">{error}</p>}
      {success && <p className="admin-success">{success}</p>}
    </div>
  );
}
