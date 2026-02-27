/**
 * Media panel — integrates LiveKit for voice, video, and screen sharing.
 *
 * Supports:
 * - Voice calls (microphone audio)
 * - Video calls (camera feed with participant grid)
 * - Screen sharing / game sharing (prominent overlay)
 * - Local self-view for camera, screen share, and mic status
 *
 * Uses @livekit/components-react for WebRTC transport.
 * LiveKit's can_publish grant covers all track sources (mic, camera, screen).
 * Video starts disabled; the user toggles camera/screen via control buttons.
 *
 * Call state lives in the voice store so the call persists across
 * tab and channel switches (like Discord).
 */

import { useCallback, useEffect, useMemo, useState } from 'react';
import {
  LiveKitRoom,
  RoomAudioRenderer,
  useParticipants,
  useTracks,
  VideoTrack,
  useLocalParticipant,
} from '@livekit/components-react';
import '@livekit/components-styles';
import { Track, type LocalParticipant } from 'livekit-client';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
import { useVoiceStore } from '@/stores/voice';
import * as api from '@/lib/api';
import { isTauri, getPlatformMediaStatus, type PlatformMediaStatus } from '@/lib/tauri';

/** Local media status bar shown above the controls. */
function LocalMediaStatus() {
  const { localParticipant } = useLocalParticipant();
  const lp = localParticipant as LocalParticipant;

  const micEnabled = lp.isMicrophoneEnabled;
  const camEnabled = lp.isCameraEnabled;
  const screenEnabled = lp.isScreenShareEnabled;

  return (
    <div className="local-media-status">
      <span className={`status-pill ${micEnabled ? 'on' : 'off'}`}>
        {micEnabled ? 'Mic ON' : 'Mic OFF'}
      </span>
      <span className={`status-pill ${camEnabled ? 'on' : 'off'}`}>
        {camEnabled ? 'Cam ON' : 'Cam OFF'}
      </span>
      {screenEnabled && (
        <span className="status-pill sharing">Sharing Screen</span>
      )}
    </div>
  );
}

/** Controls bar rendered inside the LiveKit room context. */
function MediaControls({ onLeave }: { onLeave: () => void }) {
  const { localParticipant } = useLocalParticipant();
  const lp = localParticipant as LocalParticipant;

  const micEnabled = lp.isMicrophoneEnabled;
  const camEnabled = lp.isCameraEnabled;
  const screenEnabled = lp.isScreenShareEnabled;

  // Listen for device hot-plug events during an active call.
  // Shows a transient notification so the user knows a device was added/removed.
  const [deviceNotice, setDeviceNotice] = useState<string | null>(null);
  useEffect(() => {
    if (!navigator.mediaDevices?.addEventListener) return;
    const handler = () => {
      setDeviceNotice('Audio/video device changed. Open Audio Settings to select.');
      const timer = setTimeout(() => setDeviceNotice(null), 5000);
      return () => clearTimeout(timer);
    };
    navigator.mediaDevices.addEventListener('devicechange', handler);
    return () => {
      navigator.mediaDevices.removeEventListener('devicechange', handler);
    };
  }, []);

  const toggleMic = useCallback(async () => {
    await lp.setMicrophoneEnabled(!micEnabled);
  }, [lp, micEnabled]);

  const toggleCamera = useCallback(async () => {
    await lp.setCameraEnabled(!camEnabled);
  }, [lp, camEnabled]);

  const toggleScreen = useCallback(async () => {
    await lp.setScreenShareEnabled(!screenEnabled);
  }, [lp, screenEnabled]);

  return (
    <div className="media-controls">
      {deviceNotice && (
        <div className="device-notice" role="status">{deviceNotice}</div>
      )}
      <button
        className={`media-control-btn ${micEnabled ? 'active' : 'muted'}`}
        onClick={toggleMic}
        title={micEnabled ? 'Mute microphone' : 'Unmute microphone'}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
          {micEnabled ? (
            <path d="M8 11a3 3 0 003-3V4a3 3 0 10-6 0v4a3 3 0 003 3zm5-3a5 5 0 01-4.5 4.975V15h-1v-2.025A5 5 0 013 8h1a4 4 0 108 0h1z"/>
          ) : (
            <>
              <path d="M8 11a3 3 0 003-3V4a3 3 0 10-6 0v4a3 3 0 003 3zm5-3a5 5 0 01-4.5 4.975V15h-1v-2.025A5 5 0 013 8h1a4 4 0 108 0h1z" opacity="0.3"/>
              <line x1="2" y1="2" x2="14" y2="14" stroke="currentColor" strokeWidth="1.5"/>
            </>
          )}
        </svg>
      </button>

      <button
        className={`media-control-btn ${camEnabled ? 'active' : 'muted'}`}
        onClick={toggleCamera}
        title={camEnabled ? 'Turn off camera' : 'Turn on camera'}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
          {camEnabled ? (
            <path d="M0 4.5A1.5 1.5 0 011.5 3h8A1.5 1.5 0 0111 4.5v1.05l3.15-1.8A.5.5 0 0115 4.2v7.6a.5.5 0 01-.85.35L11 10.35v1.15a1.5 1.5 0 01-1.5 1.5h-8A1.5 1.5 0 010 11.5v-7z"/>
          ) : (
            <>
              <path d="M0 4.5A1.5 1.5 0 011.5 3h8A1.5 1.5 0 0111 4.5v1.05l3.15-1.8A.5.5 0 0115 4.2v7.6a.5.5 0 01-.85.35L11 10.35v1.15a1.5 1.5 0 01-1.5 1.5h-8A1.5 1.5 0 010 11.5v-7z" opacity="0.3"/>
              <line x1="1" y1="2" x2="14" y2="14" stroke="currentColor" strokeWidth="1.5"/>
            </>
          )}
        </svg>
      </button>

      <button
        className={`media-control-btn screen-btn ${screenEnabled ? 'active sharing' : ''}`}
        onClick={toggleScreen}
        title={screenEnabled ? 'Stop sharing' : 'Share screen'}
      >
        <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
          <path d="M0 3.5A1.5 1.5 0 011.5 2h13A1.5 1.5 0 0116 3.5v7a1.5 1.5 0 01-1.5 1.5H10v1h2v1H4v-1h2v-1H1.5A1.5 1.5 0 010 10.5v-7zM1.5 3a.5.5 0 00-.5.5v7a.5.5 0 00.5.5h13a.5.5 0 00.5-.5v-7a.5.5 0 00-.5-.5h-13z"/>
          {screenEnabled && (
            <path d="M6 6h4v3H6z" opacity="0.5"/>
          )}
        </svg>
      </button>

      <div className="media-controls-divider" />

      <button onClick={onLeave} className="media-control-btn leave-call-btn" title="Leave call">
        <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
          <path d="M3.654 1.328a.678.678 0 00-1.015-.063L1.605 2.3c-.483.484-.661 1.169-.45 1.77a17.568 17.568 0 004.168 6.608 17.569 17.569 0 006.608 4.168c.601.211 1.286.033 1.77-.45l1.034-1.034a.678.678 0 00-.063-1.015l-2.307-1.794a.678.678 0 00-.58-.122l-2.19.547a1.745 1.745 0 01-1.657-.459L5.482 8.062a1.745 1.745 0 01-.46-1.657l.548-2.19a.678.678 0 00-.122-.58L3.654 1.328z"/>
        </svg>
      </button>
    </div>
  );
}

/** Local self-view: shows your own camera and screen share. */
function LocalSelfView() {
  const camTracks = useTracks([Track.Source.Camera]);
  const screenTracks = useTracks([Track.Source.ScreenShare]);
  const { localParticipant } = useLocalParticipant();

  const localCam = camTracks.find(
    (t) =>
      t.participant.identity === localParticipant.identity &&
      t.publication &&
      !t.publication.isMuted &&
      t.publication.track,
  );

  const localScreen = screenTracks.find(
    (t) =>
      t.participant.identity === localParticipant.identity &&
      t.publication &&
      !t.publication.isMuted &&
      t.publication.track,
  );

  if (!localCam && !localScreen) return null;

  return (
    <div className="local-self-view">
      {localCam && (
        <div className="self-view-tile">
          <VideoTrack trackRef={localCam} />
          <span className="self-view-label">You (camera)</span>
        </div>
      )}
      {localScreen && (
        <div className="self-view-tile screen">
          <VideoTrack trackRef={localScreen} />
          <span className="self-view-label">You (screen)</span>
        </div>
      )}
    </div>
  );
}

/** Prominent screen share display when someone else is sharing. */
function ScreenShareView() {
  const screenTracks = useTracks([Track.Source.ScreenShare]);
  const { localParticipant } = useLocalParticipant();

  // Show remote screen shares prominently; local is shown in LocalSelfView.
  const remoteShares = screenTracks.filter(
    (t) =>
      t.participant.identity !== localParticipant.identity &&
      t.publication &&
      !t.publication.isMuted &&
      t.publication.track,
  );

  if (remoteShares.length === 0) return null;

  const activeShare = remoteShares[0];

  return (
    <div className="screen-share-view">
      <div className="screen-share-header">
        <span className="screen-share-badge">LIVE</span>
        <span className="screen-share-label">
          {activeShare.participant.identity.slice(0, 12)}... is sharing
        </span>
      </div>
      <div className="screen-share-content">
        <VideoTrack trackRef={activeShare} />
      </div>
    </div>
  );
}

/** Participant grid with video tiles or audio-only avatars. */
function ParticipantGrid() {
  const participants = useParticipants();
  const micTracks = useTracks([Track.Source.Microphone]);
  const camTracks = useTracks([Track.Source.Camera]);

  const speakingIds = new Set(
    micTracks
      .filter((t) => t.publication?.isMuted === false)
      .map((t) => t.participant.identity),
  );

  const cameraByIdentity = new Map(
    camTracks
      .filter((t) => t.publication && !t.publication.isMuted)
      .map((t) => [t.participant.identity, t]),
  );

  const hasAnyVideo = cameraByIdentity.size > 0;

  return (
    <div className={`participant-grid ${hasAnyVideo ? 'has-video' : 'audio-only'}`}>
      {participants.map((p) => {
        const camTrack = cameraByIdentity.get(p.identity);
        const isSpeaking = speakingIds.has(p.identity);

        if (camTrack?.publication?.track) {
          return (
            <div
              key={p.identity}
              className={`participant-tile video ${isSpeaking ? 'speaking' : ''}`}
            >
              <VideoTrack trackRef={camTrack} />
              <span className="participant-label">
                {p.identity.slice(0, 12)}...
                {isSpeaking && <span className="speaking-indicator" />}
              </span>
            </div>
          );
        }

        return (
          <div
            key={p.identity}
            className={`participant-tile audio-tile ${isSpeaking ? 'speaking' : ''}`}
          >
            <div className="participant-avatar-circle">
              {p.identity.charAt(0).toUpperCase()}
            </div>
            <span className="participant-label">
              {p.identity.slice(0, 12)}...
              {isSpeaking && <span className="speaking-indicator" />}
            </span>
          </div>
        );
      })}
    </div>
  );
}

/** Room content rendered inside the LiveKitRoom context. */
function RoomContent({ onLeave }: { onLeave: () => void }) {
  return (
    <>
      <RoomAudioRenderer />
      <LocalMediaStatus />
      <LocalSelfView />
      <ScreenShareView />
      <ParticipantGrid />
      <MediaControls onLeave={onLeave} />
    </>
  );
}

/** Platform media warning banner (Linux PipeWire / portal issues). */
function PlatformMediaWarning({ mediaStatus }: { mediaStatus: PlatformMediaStatus | null }) {
  if (!mediaStatus || mediaStatus.warnings.length === 0) return null;
  return (
    <div className="voice-error" role="status">
      {mediaStatus.warnings.map((w, i) => (
        <p key={i} className="voice-setup-hint">{w}</p>
      ))}
    </div>
  );
}

export function VoicePanel() {
  const identity = useIdentityStore((s) => s.identity);
  const permissions = useIdentityStore((s) => s.permissions);
  const activeChannelId = useChannelsStore((s) => s.activeChannelId);
  const channels = useChannelsStore((s) => s.channels);

  const {
    voiceToken,
    livekitUrl,
    iceServers,
    connectedChannelId,
    joining,
    callActive,
    lastJoinError,
    joinCall,
    leaveCall,
    checkCallActive,
  } = useVoiceStore();

  // Query platform media capabilities once (PipeWire, xdg-desktop-portal).
  const [mediaStatus, setMediaStatus] = useState<PlatformMediaStatus | null>(null);
  useEffect(() => {
    if (!isTauri()) return;
    let cancelled = false;
    getPlatformMediaStatus()
      .then((status) => { if (!cancelled) setMediaStatus(status); })
      .catch(() => { /* non-fatal: desktop-only command */ });
    return () => { cancelled = true; };
  }, []);

  const activeChannel = channels.find((c) => c.channel_id === activeChannelId);
  const isVoiceCapable =
    activeChannel?.channel_type === 'Voice' || activeChannel?.channel_type === 'Hybrid';
  const isVoiceAllowed = permissions?.capabilities.can_voice ?? true;
  const canJoinVoice = isVoiceAllowed;

  // Poll voice status to determine if a call is active (Create vs Join).
  useEffect(() => {
    if (!isVoiceCapable || !activeChannelId || !identity?.pseudonymId || voiceToken) return;

    checkCallActive(identity.pseudonymId!, activeChannelId);
    const interval = setInterval(
      () => checkCallActive(identity.pseudonymId!, activeChannelId),
      10_000,
    );
    return () => clearInterval(interval);
  }, [isVoiceCapable, activeChannelId, identity?.pseudonymId, voiceToken, checkCallActive]);

  const pseudonymId = identity?.pseudonymId ?? null;

  const handleJoin = useCallback(async () => {
    if (!pseudonymId || !activeChannelId || !canJoinVoice) return;
    await joinCall(pseudonymId, activeChannelId);
  }, [pseudonymId, activeChannelId, canJoinVoice, joinCall]);

  const handleLeave = useCallback(async () => {
    if (!pseudonymId) return;
    await leaveCall(pseudonymId);
  }, [pseudonymId, leaveCall]);

  // Fetch setup guidance from server when error indicates voice is not configured.
  const [setupHint, setSetupHint] = useState<string | null>(null);
  useEffect(() => {
    if (!lastJoinError?.includes('not configured')) {
      setSetupHint(null);
      return;
    }

    let cancelled = false;

    api.getVoiceConfigStatus()
      .then((status) => {
        if (cancelled || (status.policy_enabled && status.infrastructure_ready)) return;
        setSetupHint(status.setup_hint);
      })
      .catch(() => {
        // Best-effort: if the status endpoint fails, just show the raw error
      });

    return () => {
      cancelled = true;
    };
  }, [lastJoinError]);

  // Build RTC configuration with server-provided ICE servers for NAT traversal.
  const roomOptions = useMemo(() => {
    if (!iceServers || iceServers.length === 0) return undefined;
    return {
      rtcConfig: {
        iceServers: iceServers.map((s) => ({
          urls: s.urls,
          username: s.username || undefined,
          credential: s.credential || undefined,
        })),
      },
    };
  }, [iceServers]);

  // If connected to a call, always show the LiveKitRoom (even on non-voice channels).
  if (voiceToken && livekitUrl && connectedChannelId) {
    // Find the channel name for the connected call
    const connectedChannel = channels.find((c) => c.channel_id === connectedChannelId);
    const channelLabel = connectedChannel?.name ?? connectedChannelId.slice(0, 12);

    return (
      <div className="voice-panel connected">
        <div className="voice-connected-header">
          Voice Connected — <strong>{channelLabel}</strong>
        </div>
        <LiveKitRoom
          serverUrl={livekitUrl}
          token={voiceToken}
          connect={true}
          audio={true}
          video={false}
          options={roomOptions}
        >
          <RoomContent onLeave={handleLeave} />
        </LiveKitRoom>
      </div>
    );
  }

  // Only show the join button on voice-capable channels
  if (!isVoiceCapable || !activeChannelId) return null;

  const buttonText = joining
    ? 'Joining...'
    : callActive
      ? 'Join Call'
      : 'Create Call';

  const unavailableReason = !isVoiceAllowed
    ? 'Voice is disabled by server policy for your identity.'
    : null;

  return (
    <div className="voice-panel disconnected">
      <PlatformMediaWarning mediaStatus={mediaStatus} />
      <button
        onClick={handleJoin}
        disabled={joining || !canJoinVoice}
        className="voice-join-btn"
        title={unavailableReason ?? undefined}
      >
        {buttonText}
      </button>
      {(lastJoinError || unavailableReason) && (
        <div className="voice-error" role="alert">
          {setupHint ? (
            <>
              <p>Voice is not configured on this server.</p>
              <p className="voice-setup-hint">{setupHint}</p>
            </>
          ) : (
            lastJoinError ?? unavailableReason
          )}
        </div>
      )}
    </div>
  );
}
