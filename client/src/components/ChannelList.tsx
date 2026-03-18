/**
 * Channel list sidebar component.
 *
 * Shows available channels with join/leave controls, allows selecting
 * the active channel, and provides a create button for moderators.
 */

import { useEffect, useState } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import { CreateChannelDialog } from '@/components/CreateChannelDialog';
import { createInviteLink } from '@/lib/invite';
import { getApiBaseUrl } from '@/lib/api';
import type { Channel } from '@/types';

const CHANNEL_TYPE_ICONS: Record<string, { icon: string; tooltip: string }> = {
  Text: { icon: '#', tooltip: 'Text channel — chat with messages' },
  Voice: { icon: '🔊', tooltip: 'Voice channel — real-time audio/video with text chat' },
  Hybrid: { icon: '🔊', tooltip: 'Voice channel — real-time audio/video with text chat' },
  Agent: { icon: '🤖', tooltip: 'Agent channel — AI agents can participate here' },
  Broadcast: { icon: '📢', tooltip: 'Broadcast channel — announcements from moderators' },
};

const DEFAULT_CHANNEL_ICON = { icon: '#', tooltip: 'Channel' };

function ChannelItem({
  channel,
  active,
  pseudonymId,
  onSelect,
}: {
  channel: Channel;
  active: boolean;
  pseudonymId: string;
  onSelect: () => void;
}) {
  const { joinChannel, leaveChannel, loadChannels } = useChannelsStore();
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);

  const handleJoin = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setBusy(true);
    setActionError(null);
    try {
      await joinChannel(pseudonymId, channel.channel_id);
      await loadChannels(pseudonymId);
    } catch (err) {
      setActionError(err instanceof Error ? err.message : 'Failed to join channel');
    } finally {
      setBusy(false);
    }
  };

  const handleLeave = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setBusy(true);
    setActionError(null);
    try {
      await leaveChannel(pseudonymId, channel.channel_id);
      await loadChannels(pseudonymId);
    } catch (err) {
      setActionError(err instanceof Error ? err.message : 'Failed to leave channel');
    } finally {
      setBusy(false);
    }
  };

  const handleCopyInvite = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setActionError(null);
    try {
      const apiBase = getApiBaseUrl();
      const { url } = await createInviteLink(apiBase, pseudonymId);
      await navigator.clipboard.writeText(url);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch (err) {
      setActionError(err instanceof Error ? err.message : 'Failed to copy invite link');
    }
  };

  return (
    <div className={`channel-item ${active ? 'active' : ''}`}>
      <button className="channel-select" onClick={onSelect}>
        <span className="channel-icon" title={(CHANNEL_TYPE_ICONS[channel.channel_type] ?? DEFAULT_CHANNEL_ICON).tooltip}>
          {(CHANNEL_TYPE_ICONS[channel.channel_type] ?? DEFAULT_CHANNEL_ICON).icon}
        </span>
        <span className="channel-name">{channel.name}</span>
        {channel.federation_scope === 'Federated' && (
          <span className="federation-badge" title="Federated — messages in this channel are shared with connected partner servers">
            F
          </span>
        )}
      </button>
      <div className="channel-actions">
        <button
          className="channel-action-btn invite-btn"
          onClick={handleCopyInvite}
          title={copied ? 'Copied!' : 'Copy invite link'}
        >
          {copied ? '!' : 'i'}
        </button>
        {active ? (
          <button
            className="channel-action-btn leave-btn"
            onClick={handleLeave}
            disabled={busy}
            title="Leave channel"
          >
            x
          </button>
        ) : (
          <button
            className="channel-action-btn join-btn"
            onClick={handleJoin}
            disabled={busy}
            title="Join channel"
          >
            +
          </button>
        )}
      </div>
      {actionError && (
        <div className="channel-action-error" role="alert">
          <span>{actionError}</span>
          <button
            onClick={(e) => { e.stopPropagation(); setActionError(null); }}
            className="channel-error-dismiss"
            aria-label="Dismiss"
          >
            &times;
          </button>
        </div>
      )}
    </div>
  );
}

export function ChannelList() {
  const identity = useIdentityStore((s) => s.identity);
  const permissions = useIdentityStore((s) => s.permissions);
  const {
    channels,
    activeChannelId,
    loading,
    error,
    loadChannels,
    selectChannel,
  } = useChannelsStore();
  const [showCreate, setShowCreate] = useState(false);

  useEffect(() => {
    if (identity?.pseudonymId) {
      loadChannels(identity.pseudonymId);
    }
  }, [identity?.pseudonymId, loadChannels]);

  if (!identity?.pseudonymId) return null;

  const handleSelect = (channelId: string) => {
    selectChannel(identity.pseudonymId!, channelId);
  };

  if (loading) {
    return <div className="channel-list loading">Loading channels...</div>;
  }

  if (error) {
    return (
      <div className="channel-list loading">
        <p className="error-message">{error}</p>
        <button className="primary-btn" onClick={() => loadChannels(identity.pseudonymId!)}>
          Retry
        </button>
      </div>
    );
  }

  return (
    <nav className="channel-list">
      <div className="channel-list-header">
        <h3>Channels</h3>
        {permissions?.capabilities.can_moderate && (
          <button
            className="create-channel-btn"
            onClick={() => setShowCreate(true)}
            title="Create channel"
          >
            +
          </button>
        )}
      </div>
      {channels.length === 0 && (
        <p className="no-channels">No channels available</p>
      )}
      {channels.map((ch) => (
        <ChannelItem
          key={ch.channel_id}
          channel={ch}
          active={activeChannelId === ch.channel_id}
          pseudonymId={identity.pseudonymId!}
          onSelect={() => handleSelect(ch.channel_id)}
        />
      ))}
      {showCreate && <CreateChannelDialog onClose={() => setShowCreate(false)} />}
    </nav>
  );
}
