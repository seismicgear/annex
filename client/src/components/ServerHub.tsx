/**
 * Server Hub — the client-side node hub sidebar.
 *
 * Renders the user's local database of established Merkle tree insertions
 * as a vertical icon list (like Discord's server bar). Each icon represents
 * an established cryptographic identity on a remote server node.
 *
 * Click-to-connect: immediate UI transition with async crypto handshake.
 * Federation hopping: "+" to discover and join new servers.
 */

import { useState, useCallback } from 'react';
import { useServersStore } from '@/stores/servers';
import { resolveUrl } from '@/lib/api';
import type { SavedServer } from '@/types';

interface AddServerDialogProps {
  onClose: () => void;
  onAdd: (baseUrl: string) => Promise<void>;
}

function AddServerDialog({ onClose, onAdd }: AddServerDialogProps) {
  const [url, setUrl] = useState('');
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState('');

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = url.trim();
    if (!trimmed) return;

    // Normalize and validate URL
    let baseUrl = trimmed;
    if (!/^https?:\/\//i.test(baseUrl)) {
      baseUrl = `https://${baseUrl}`;
    }
    try {
      const parsed = new URL(baseUrl);
      // Block non-HTTP protocols that could have been injected
      if (!['http:', 'https:'].includes(parsed.protocol)) {
        setError('Only http and https URLs are supported.');
        return;
      }
    } catch {
      setError('Invalid URL format.');
      return;
    }

    setAdding(true);
    setError('');
    try {
      await onAdd(baseUrl);
      onClose();
    } catch {
      setError('Could not reach server. Check the URL and try again.');
    } finally {
      setAdding(false);
    }
  };

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog add-server-dialog" onClick={(e) => e.stopPropagation()}>
        <h3>Join a Server</h3>
        <p className="dialog-description">
          Enter the URL of an Annex server to establish a new cryptographic identity there.
        </p>
        <form onSubmit={handleSubmit}>
          <label>
            Server URL
            <input
              type="text"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              placeholder="annex.example.com"
              autoFocus
            />
          </label>
          {error && <p className="form-error">{error}</p>}
          <div className="dialog-actions">
            <button type="button" onClick={onClose}>Cancel</button>
            <button type="submit" className="primary-btn" disabled={adding || !url.trim()}>
              {adding ? 'Connecting...' : 'Join Server'}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

function ServerIcon({ server, isActive, imageUrl, onClick }: {
  server: SavedServer;
  isActive: boolean;
  imageUrl?: string | null;
  onClick: () => void;
}) {
  const initial = server.label.charAt(0).toUpperCase();
  const memberCount = server.cachedSummary?.total_active_members;

  return (
    <div className="server-hub-item-wrapper">
      <div className={`server-hub-pill ${isActive ? 'active' : ''}`} />
      <button
        className={`server-hub-icon ${isActive ? 'active' : ''} ${imageUrl ? 'has-image' : ''}`}
        style={{
          '--server-accent': server.accentColor,
        } as React.CSSProperties}
        onClick={onClick}
        title={`${server.label}${server.slug ? ` (${server.slug})` : ''}${memberCount ? ` — ${memberCount} online` : ''}`}
      >
        {imageUrl ? (
          <img src={resolveUrl(imageUrl)} alt={server.label} className="server-hub-image" />
        ) : (
          <span className="server-hub-initial">{initial}</span>
        )}
      </button>
    </div>
  );
}

export function ServerHub() {
  const servers = useServersStore((s) => s.servers);
  const activeServerId = useServersStore((s) => s.activeServerId);
  const switching = useServersStore((s) => s.switching);
  const switchServer = useServersStore((s) => s.switchServer);
  const addRemoteServer = useServersStore((s) => s.addRemoteServer);
  const serverImageUrl = useServersStore((s) => s.serverImageUrl);
  const [showAddDialog, setShowAddDialog] = useState(false);

  const handleAdd = useCallback(async (baseUrl: string) => {
    const server = await addRemoteServer(baseUrl);
    if (!server) throw new Error('Failed to add server');
  }, [addRemoteServer]);

  if (servers.length === 0) return null;

  return (
    <>
      <nav className={`server-hub ${switching ? 'switching' : ''}`}>
        <div className="server-hub-list">
          {servers.map((server) => (
            <ServerIcon
              key={server.id}
              server={server}
              isActive={server.id === activeServerId}
              imageUrl={server.id === activeServerId ? serverImageUrl : null}
              onClick={() => switchServer(server.id)}
            />
          ))}
        </div>

        <div className="server-hub-separator" />

        <button
          className="server-hub-icon add-server-btn"
          onClick={() => setShowAddDialog(true)}
          title="Join a server"
        >
          <svg width="20" height="20" viewBox="0 0 20 20" fill="currentColor">
            <path d="M10 3a1 1 0 011 1v5h5a1 1 0 110 2h-5v5a1 1 0 11-2 0v-5H4a1 1 0 110-2h5V4a1 1 0 011-1z" />
          </svg>
        </button>
      </nav>

      {showAddDialog && (
        <AddServerDialog
          onClose={() => setShowAddDialog(false)}
          onAdd={handleAdd}
        />
      )}
    </>
  );
}
