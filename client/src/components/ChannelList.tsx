/**
 * Channel list sidebar component.
 *
 * Shows available channels, allows joining, and lets the user select
 * which channel to view. Channels are grouped by type.
 */

import { useEffect } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
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
  onSelect,
}: {
  channel: Channel;
  active: boolean;
  onSelect: () => void;
}) {
  return (
    <button
      className={`channel-item ${active ? 'active' : ''}`}
      onClick={onSelect}
    >
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
  );
}

export function ChannelList() {
  const identity = useIdentityStore((s) => s.identity);
  const {
    channels,
    activeChannelId,
    loading,
    loadChannels,
    selectChannel,
  } = useChannelsStore();

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
      <h3>Channels</h3>
      {channels.length === 0 && (
        <p className="no-channels">No channels available</p>
      )}
      {channels.map((ch) => (
        <ChannelItem
          key={ch.channel_id}
          channel={ch}
          active={activeChannelId === ch.channel_id}
          onSelect={() => handleSelect(ch.channel_id)}
        />
      ))}
    </nav>
  );
}
