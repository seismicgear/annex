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
  const pseudonymId = useIdentityStore((s) => s.identity?.pseudonymId ?? null);
  const canModerate = useIdentityStore((s) => s.permissions?.capabilities.can_moderate ?? false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (!activeChannelId) return null;

  if (e2e) {
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
