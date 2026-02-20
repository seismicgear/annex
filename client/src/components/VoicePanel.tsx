/**
 * Voice panel component — integrates LiveKit for voice channels.
 *
 * Shows join/leave controls and a participant list when connected.
 * Uses @livekit/components-react for audio rendering.
 */

import { useState, useCallback } from 'react';
import {
  LiveKitRoom,
  RoomAudioRenderer,
  useParticipants,
  useTracks,
} from '@livekit/components-react';
import '@livekit/components-styles';
import { Track } from 'livekit-client';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
import * as api from '@/lib/api';

function VoiceParticipants() {
  const participants = useParticipants();
  const tracks = useTracks([Track.Source.Microphone]);
  const speakingIds = new Set(
    tracks.filter((t) => t.publication?.isMuted === false).map((t) => t.participant.identity),
  );

  return (
    <div className="voice-participants">
      {participants.map((p) => (
        <div
          key={p.identity}
          className={`voice-participant ${speakingIds.has(p.identity) ? 'speaking' : ''}`}
        >
          <span className="participant-name">{p.identity.slice(0, 12)}...</span>
          {speakingIds.has(p.identity) && <span className="speaking-indicator" />}
        </div>
      ))}
    </div>
  );
}

export function VoicePanel() {
  const identity = useIdentityStore((s) => s.identity);
  const activeChannelId = useChannelsStore((s) => s.activeChannelId);
  const channels = useChannelsStore((s) => s.channels);

  const [voiceToken, setVoiceToken] = useState<string | null>(null);
  const [livekitUrl, setLivekitUrl] = useState<string | null>(null);
  const [joining, setJoining] = useState(false);

  const activeChannel = channels.find((c) => c.channel_id === activeChannelId);
  const isVoiceCapable =
    activeChannel?.channel_type === 'Voice' || activeChannel?.channel_type === 'Hybrid';

  const handleJoin = useCallback(async () => {
    if (!identity?.pseudonymId || !activeChannelId) return;
    setJoining(true);
    try {
      const { token, url } = await api.joinVoice(identity.pseudonymId, activeChannelId);
      setVoiceToken(token);
      setLivekitUrl(url);
    } catch (e) {
      console.error('Failed to join voice:', e);
    } finally {
      setJoining(false);
    }
  }, [identity?.pseudonymId, activeChannelId]);

  const handleLeave = useCallback(async () => {
    if (!identity?.pseudonymId || !activeChannelId) return;
    try {
      await api.leaveVoice(identity.pseudonymId, activeChannelId);
    } catch {
      // Best effort
    }
    setVoiceToken(null);
    setLivekitUrl(null);
  }, [identity?.pseudonymId, activeChannelId]);

  if (!isVoiceCapable || !activeChannelId) return null;

  if (voiceToken && livekitUrl) {
    return (
      <div className="voice-panel connected">
        <LiveKitRoom
          serverUrl={livekitUrl}
          token={voiceToken}
          connect={true}
          audio={true}
          video={false}
        >
          <RoomAudioRenderer />
          <VoiceParticipants />
          <button onClick={handleLeave} className="voice-leave-btn">
            Leave Voice
          </button>
        </LiveKitRoom>
      </div>
    );
  }

  return (
    <div className="voice-panel disconnected">
      <button onClick={handleJoin} disabled={joining} className="voice-join-btn">
        {joining ? 'Joining...' : 'Join Voice'}
      </button>
    </div>
  );
}
