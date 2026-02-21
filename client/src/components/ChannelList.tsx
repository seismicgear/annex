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
import { generateInviteLink } from '@/lib/invite';
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
  serverSlug,
  onSelect,
}: {
  channel: Channel;
  active: boolean;
  pseudonymId: string;
  serverSlug: string;
  onSelect: () => void;
}) {
  const { joinChannel, leaveChannel, loadChannels } = useChannelsStore();
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState(false);

  const handleJoin = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setBusy(true);
    try {
      await joinChannel(pseudonymId, channel.channel_id);
      await loadChannels(pseudonymId);
    } catch {
      // Join failed — user can retry
    } finally {
      setBusy(false);
    }
  };

  const handleLeave = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setBusy(true);
    try {
      await leaveChannel(pseudonymId, channel.channel_id);
      await loadChannels(pseudonymId);
    } catch {
      // Leave failed — user can retry
    } finally {
      setBusy(false);
    }
  };

  const handleCopyInvite = async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      const link = generateInviteLink(channel.channel_id, serverSlug, channel.name);
      await navigator.clipboard.writeText(link);
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } catch {
      // Clipboard access denied or insecure context
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
          serverSlug={identity.serverSlug}
          onSelect={() => handleSelect(ch.channel_id)}
        />
      ))}
      {showCreate && <CreateChannelDialog onClose={() => setShowCreate(false)} />}
    </nav>
  );
}
