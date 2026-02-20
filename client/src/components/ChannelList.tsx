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
import type { Channel, ChannelType } from '@/types';

const CHANNEL_TYPE_ICONS: Record<ChannelType, string> = {
  TEXT: '#',
  VOICE: '🔊',
  HYBRID: '#🔊',
  AGENT: '🤖',
  BROADCAST: '📢',
};

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

  const handleJoin = async (e: React.MouseEvent) => {
    e.stopPropagation();
    setBusy(true);
    try {
      await joinChannel(pseudonymId, channel.channel_id);
      await loadChannels(pseudonymId);
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
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={`channel-item ${active ? 'active' : ''}`}>
      <button className="channel-select" onClick={onSelect}>
        <span className="channel-icon">
          {CHANNEL_TYPE_ICONS[channel.channel_type]}
        </span>
        <span className="channel-name">{channel.name}</span>
        {channel.federation_scope === 'FEDERATED' && (
          <span className="federation-badge" title="Federated channel">
            F
          </span>
        )}
      </button>
      <div className="channel-actions">
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
        {permissions?.can_moderate && (
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
