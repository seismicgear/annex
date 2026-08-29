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
import { normalizeServerUrl } from '@/lib/url';
import { useIdentityStore } from '@/stores/identity';
import type { SavedServer } from '@/types';
import { Modal } from '@/components/Modal';
import { useDialogTitleId } from '@/lib/use-dialog-title-id';

interface AddServerDialogProps {
  onClose: () => void;
  onAdd: (baseUrl: string) => Promise<void>;
}

function AddServerDialog({ onClose, onAdd }: AddServerDialogProps) {
  const titleId = useDialogTitleId();
  const [url, setUrl] = useState('');
  const [adding, setAdding] = useState(false);
  const [error, setError] = useState('');

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = url.trim();
    if (!trimmed) return;

    // Normalize and validate URL
    let baseUrl: string;
    try {
      baseUrl = normalizeServerUrl(trimmed);
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
      setError(`Could not reach server at ${baseUrl}. Check the URL and try again.`);
    } finally {
      setAdding(false);
    }
  };

  return (
    <Modal onClose={onClose} className="add-server-dialog" titleId={titleId}>
      <h2 id={titleId}>Join a Server</h2>
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
        {error && <p className="form-error" role="alert">{error}</p>}
        <div className="dialog-actions">
          <button type="button" onClick={onClose}>Cancel</button>
          <button type="submit" className="primary-btn" disabled={adding || !url.trim()}>
            {adding ? 'Connecting...' : 'Join Server'}
          </button>
        </div>
      </form>
    </Modal>
  );
}

function ServerIcon({ server, isActive, imageUrl, onClick, onRetry, onRemove }: {
  server: SavedServer;
  isActive: boolean;
  imageUrl?: string | null;
  onClick: () => void;
  onRetry?: () => void;
  onRemove?: () => void;
}) {
  const initial = server.label.charAt(0).toUpperCase();
  const memberCount = server.cachedSummary?.total_active_members;
  const isPending = !server.identityId;
  const phase = useIdentityStore((s) => s.phase);
  // A placeholder is "failed" when registration is no longer in progress
  const isFailed = isPending && phase !== 'keys_ready' && phase !== 'registering' && phase !== 'proving' && phase !== 'verifying';

  return (
    <div className="server-hub-item-wrapper">
      <div className={`server-hub-pill ${isActive ? 'active' : ''}`} />
      <button
        className={`server-hub-icon ${isActive ? 'active' : ''} ${imageUrl ? 'has-image' : ''} ${isPending ? 'pending' : ''} ${isFailed ? 'failed' : ''}`}
        style={{
          '--server-accent': server.accentColor,
          ...(isPending && !isFailed ? { opacity: 0.5 } : {}),
          ...(isFailed ? { opacity: 0.6 } : {}),
        } as React.CSSProperties}
        onClick={isFailed ? undefined : onClick}
        disabled={isPending && !isFailed}
        title={isFailed
          ? `${server.label} — registration failed (right-click to retry or remove)`
          : isPending
            ? `${server.label} — registration pending`
            : `${server.label}${server.slug ? ` (${server.slug})` : ''}${memberCount ? ` — ${memberCount} online` : ''}`}
      >
        {imageUrl ? (
          <img src={resolveUrl(imageUrl)} alt={server.label} className="server-hub-image" />
        ) : (
          <span className="server-hub-initial">{isFailed ? '!' : initial}</span>
        )}
      </button>
      {isFailed && (
        <div className="server-hub-failed-actions">
          {onRetry && (
            <button className="server-hub-retry-btn" onClick={onRetry} title="Retry registration">
              Retry
            </button>
          )}
          {onRemove && (
            <button className="server-hub-remove-btn" onClick={onRemove} title="Remove server">
              &times;
            </button>
          )}
        </div>
      )}
    </div>
  );
}

export function ServerHub() {
  const servers = useServersStore((s) => s.servers);
  const activeServerId = useServersStore((s) => s.activeServerId);
  const switching = useServersStore((s) => s.switching);
  const switchServer = useServersStore((s) => s.switchServer);
  const switchError = useServersStore((s) => s.switchError);
  const clearSwitchError = useServersStore((s) => s.clearSwitchError);
  const beginRemoteRegistration = useServersStore((s) => s.beginRemoteRegistration);
  const removeServer = useServersStore((s) => s.removeServer);
  const serverImageUrl = useServersStore((s) => s.serverImageUrl);
  const [showAddDialog, setShowAddDialog] = useState(false);

  const handleSwitch = useCallback(async (serverId: string) => {
    try {
      await switchServer(serverId);
    } catch {
      // Error is captured in switchError state — no need to re-throw
    }
  }, [switchServer]);

  const handleAdd = useCallback(async (baseUrl: string) => {
    const server = await beginRemoteRegistration(baseUrl);
    if (!server) throw new Error('Failed to add server');
  }, [beginRemoteRegistration]);

  const handleRetry = useCallback(async (server: SavedServer) => {
    // Remove the stale placeholder and re-initiate registration
    await removeServer(server.id).catch(() => {});
    const newServer = await beginRemoteRegistration(server.baseUrl);
    if (!newServer) throw new Error('Failed to retry registration');
  }, [beginRemoteRegistration, removeServer]);

  const handleRemove = useCallback(async (serverId: string) => {
    await removeServer(serverId);
  }, [removeServer]);

  if (servers.length === 0) return null;

  return (
    <>
      {/* Not inside the rail. It used to be: a `role="alert"` wrapping the
          single character "!", with the real message in a `title`. That
          announced "!" to a screen reader, was invisible to touch (a title
          has no tap), had no CSS rule of its own so it rendered unstyled,
          and never said the thing that matters most — that the switch rolled
          back and you are still where you started. The rail is 72px with
          `overflow-x: hidden`, so prose cannot live in it; this sits above
          the app instead, where a sentence fits. */}
      {switchError && (
        <div className="server-switch-error" role="alert">
          <span aria-hidden="true">&#9888;&#65039;</span>
          <span>
            Couldn&apos;t switch servers: {switchError}. You&apos;re still on the
            server you started from.
          </span>
          <button
            type="button"
            className="server-switch-error-dismiss"
            onClick={clearSwitchError}
            aria-label="Dismiss"
          >
            &times;
          </button>
        </div>
      )}
      <nav className={`server-hub ${switching ? 'switching' : ''}`} aria-label="Your servers">
        <div className="server-hub-list">
          {servers.map((server) => (
            <ServerIcon
              key={server.id}
              server={server}
              isActive={server.id === activeServerId}
              imageUrl={server.id === activeServerId ? serverImageUrl : null}
              onClick={() => handleSwitch(server.id)}
              onRetry={() => handleRetry(server)}
              onRemove={() => handleRemove(server.id)}
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
