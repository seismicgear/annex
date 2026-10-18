import { HostedImage } from './HostedAsset';
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
import { describeServerUrlError, normalizeServerUrl } from '@/lib/url';
import { useIdentityStore } from '@/stores/identity';
import type { InvitePayload, SavedServer } from '@/types';
import { parseInviteLink } from '@/lib/invite';
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

    // Normalize and validate URL. The wording lives in `describeServerUrlError`
    // because `StartupModeSelector` asks the same question on first run and
    // both dialogs had the same gap: a bare "Invalid URL format." that named
    // nothing the user typed.
    let baseUrl: string;
    try {
      baseUrl = parseInviteLink(trimmed) ? trimmed : normalizeServerUrl(trimmed);
    } catch (err) {
      setError(err instanceof Error && /invite/i.test(err.message) ? err.message : describeServerUrlError(trimmed, err));
      return;
    }

    setAdding(true);
    setError('');
    try {
      await onAdd(baseUrl);
      onClose();
    } catch (err) {
      // `onAdd` throws the reason the registration actually failed, which is
      // not always reachability — a local identity clone can fail with the
      // network perfectly fine. Overwriting it with "Could not reach server"
      // put the guess back that the layer below had just stopped making.
      setError(
        err instanceof Error && err.message
          ? err.message
          : `Could not reach server at ${baseUrl}. Check the URL and try again.`,
      );
    } finally {
      setAdding(false);
    }
  };

  return (
    <Modal onClose={onClose} className="add-server-dialog" titleId={titleId}>
      <h2 id={titleId}>Join a Server</h2>
      <p className="dialog-description">
        Paste an invite link or enter a server address to join.
      </p>
      <form onSubmit={handleSubmit}>
        <label>
          Server URL or invite link
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

function ServerIcon({ server, isActive, imageUrl, onClick, onRetry, onRemove, actionPending }: {
  server: SavedServer;
  isActive: boolean;
  imageUrl?: string | null;
  onClick: () => void;
  onRetry?: () => void;
  onRemove?: () => void;
  actionPending: boolean;
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
        onClick={isFailed ? onRetry : onClick}
        disabled={(isPending && !isFailed) || actionPending}
        aria-label={isFailed ? `Retry registration for ${server.label}` : server.label}
        title={isFailed
          ? `${server.label} — registration failed (click to retry)`
          : isPending
            ? `${server.label} — registration pending`
            : `${server.label}${server.slug ? ` (${server.slug})` : ''}${memberCount ? ` — ${memberCount} online` : ''}`}
      >
        {imageUrl ? (
          <HostedImage src={resolveUrl(imageUrl)} alt={server.label} className="server-hub-image" />
        ) : (
          <span className="server-hub-initial">{isFailed ? '!' : initial}</span>
        )}
      </button>
      {isFailed && (
        <div className="server-hub-failed-actions">
          {onRetry && (
            <button className="server-hub-retry-btn" onClick={onRetry} title="Retry registration" aria-label="Retry registration" disabled={actionPending}>
              Retry
            </button>
          )}
          {onRemove && (
            <button className="server-hub-remove-btn" onClick={onRemove} title="Remove server" aria-label="Remove server" disabled={actionPending}>
              &times;
            </button>
          )}
        </div>
      )}
    </div>
  );
}

export function ServerHub({ onJoinInvite }: { onJoinInvite?: (invite: InvitePayload) => Promise<void> } = {}) {
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
  const [actionPending, setActionPending] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const handleSwitch = useCallback(async (serverId: string) => {
    try {
      await switchServer(serverId);
    } catch {
      // Error is captured in switchError state — no need to re-throw
    }
  }, [switchServer]);

  const handleAdd = useCallback(async (baseUrl: string) => {
    const invite = parseInviteLink(baseUrl);
    if (invite) {
      if (!onJoinInvite) throw new Error('Invite joining is unavailable. Reopen Annex and try again.');
      await onJoinInvite(invite);
      return;
    }
    const server = await beginRemoteRegistration(baseUrl);
    if (!server) {
      throw new Error(
        useServersStore.getState().registrationError ?? 'Failed to add server',
      );
    }
  }, [beginRemoteRegistration, onJoinInvite]);

  const handleRetry = useCallback(async (server: SavedServer) => {
    setActionPending(true);
    setActionError(null);
    try {
      await removeServer(server.id);
      const newServer = await beginRemoteRegistration(server.baseUrl);
      if (!newServer) {
        throw new Error(useServersStore.getState().registrationError ?? 'Failed to retry registration');
      }
    } catch (err) {
      setActionError(`Could not retry ${server.label}: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setActionPending(false);
    }
  }, [beginRemoteRegistration, removeServer]);

  const handleRemove = useCallback(async (serverId: string) => {
    setActionPending(true);
    setActionError(null);
    try {
      await removeServer(serverId);
    } catch (err) {
      setActionError(`Could not remove server: ${err instanceof Error ? err.message : String(err)}`);
    } finally {
      setActionPending(false);
    }
  }, [removeServer]);

  if (servers.length === 0 && !actionError) return null;

  return (
    <>
      {actionError && (
        <div className="server-switch-error" role="alert">
          <span>{actionError}</span>
          <button type="button" onClick={() => setActionError(null)} aria-label="Dismiss">&times;</button>
        </div>
      )}
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
              actionPending={actionPending}
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
