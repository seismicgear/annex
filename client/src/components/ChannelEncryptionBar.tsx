/**
 * A slim bar above the message list showing whether the active channel is
 * end-to-end encrypted, and — for moderators — a control to turn it on.
 */

import { useState } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';

export function ChannelEncryptionBar() {
  const activeChannelId = useChannelsStore((s) => s.activeChannelId);
  const e2e = useChannelsStore((s) => s.activeChannelE2e);
  const enableChannelE2e = useChannelsStore((s) => s.enableChannelE2e);
  const keyState = useChannelsStore((s) => s.activeChannelKeyState);
  const keyError = useChannelsStore((s) => s.activeChannelKeyError);
  const retryChannelKey = useChannelsStore((s) => s.retryChannelKey);
  const pseudonymId = useIdentityStore((s) => s.identity?.pseudonymId ?? null);
  const canModerate = useIdentityStore((s) => s.permissions?.capabilities.can_moderate ?? false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const onRetryKey = async () => {
    if (!pseudonymId || busy) return;
    setBusy(true);
    try {
      await retryChannelKey(pseudonymId);
    } finally {
      setBusy(false);
    }
  };

  if (!activeChannelId) return null;

  if (e2e) {
    // Being unable to read an encrypted channel has two causes and they need
    // different sentences. Both used to render as the reassuring bar below,
    // above a column of "🔒 encrypted message (no key)" with no explanation
    // of why, whether it would resolve, or what to do.
    if (keyState === 'pending') {
      return (
        <div className="channel-encryption-bar key-pending" role="status">
          <span aria-hidden="true">🔑</span>
          <span>
            Encrypted, and you don&apos;t have this channel&apos;s key yet. A member who
            has it will pass it to you automatically the next time they open the
            channel — then these messages become readable.
          </span>
          <button type="button" onClick={onRetryKey} disabled={busy}>
            {busy ? 'Checking…' : 'Check again'}
          </button>
        </div>
      );
    }

    if (keyState === 'failed') {
      return (
        <div className="channel-encryption-bar key-failed" role="alert">
          <span aria-hidden="true">⚠️</span>
          <span>
            Encrypted, but the channel key could not be loaded
            {keyError ? `: ${keyError}` : ''}. Messages stay unreadable and
            sending is blocked until it succeeds.
          </span>
          <button type="button" onClick={onRetryKey} disabled={busy}>
            {busy ? 'Retrying…' : 'Retry'}
          </button>
        </div>
      );
    }

    return (
      <div className="channel-encryption-bar encrypted" role="status">
        <span aria-hidden="true">🔒</span>
        <span>End-to-end encrypted — the server can&apos;t read these messages.</span>
      </div>
    );
  }

  if (!canModerate) return null;

  const onEnable = async () => {
    if (!pseudonymId || busy) return;
    setBusy(true);
    setError(null);
    try {
      await enableChannelE2e(pseudonymId);
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to enable encryption.');
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="channel-encryption-bar" role="region" aria-label="Channel encryption">
      <span>
        Messages in this channel are stored on the server. Turn on end-to-end
        encryption so only members can read them.
      </span>
      <button type="button" onClick={onEnable} disabled={busy}>
        {busy ? 'Enabling…' : '🔒 Enable encryption'}
      </button>
      {error && <span className="channel-encryption-error">{error}</span>}
    </div>
  );
}
