/**
 * Participant visualisation for an in-progress call.
 *
 * A single responsive grid of equal tiles: your own camera (or avatar), your
 * screen share if active, one tile per remote camera the SFU forwards, and an
 * avatar tile for any audio-only remote. Each tile shows the name, a speaking
 * ring, and a camera-off avatar fallback — so the call looks like a real
 * conferencing grid rather than stacked green boxes.
 */

import { useEffect, useRef } from 'react';
import { TrackSource, type WebRtcSession } from '@/lib/webrtc';

/** Renders a MediaStreamTrack into a <video>. `mirror` flips the local camera. */
function VideoTile({ track, mirror = false }: { track: MediaStreamTrack; mirror?: boolean }) {
  const ref = useRef<HTMLVideoElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.srcObject = new MediaStream([track]);
    return () => { el.srcObject = null; };
  }, [track]);
  return (
    <video
      ref={ref}
      autoPlay
      playsInline
      muted
      className="tile-video"
      style={mirror ? { transform: 'scaleX(-1)' } : undefined}
    />
  );
}

function Avatar({ label }: { label: string }) {
  const ch = (label.trim()[0] || '?').toUpperCase();
  return <div className="tile-avatar">{ch}</div>;
}

function shortName(id: string): string {
  if (!id) return 'Participant';
  return id.length > 14 ? `${id.slice(0, 14)}…` : id;
}

// Backwards-compatible no-ops: the unified grid below now renders local +
// remote + screen, so the old separate self-view / screen-share stages are gone.
export function LocalSelfView() { return null; }
export function ScreenShareView() { return null; }

/**
 * Unified participant grid: local camera/avatar, local screen share, every
 * remote camera forwarded by the SFU, plus avatar tiles for audio-only remotes.
 */
export function ParticipantGrid({ session }: { session: WebRtcSession }) {
  const identity = session.identity;
  const isSpeaking = session.isSpeaking;

  const camPub = session.trackPublications.get(TrackSource.Camera);
  const screenPub = session.trackPublications.get(TrackSource.ScreenShare);
  const localCamTrack = camPub && !camPub.isMuted && camPub.track ? camPub.track.mediaStreamTrack : null;
  const localScreenTrack = screenPub && !screenPub.isMuted && screenPub.track ? screenPub.track.mediaStreamTrack : null;

  // Remote cameras the SFU forwards. The SFU pre-creates a video slot per peer
  // that is muted until that peer publishes — only show tiles that are live.
  const remoteVideos = (session.remoteVideoTracks ?? []).filter(
    (v) => v.track.readyState === 'live' && !v.track.muted,
  );
  // Audio-only remotes: incoming audio tracks beyond the ones that also have video.
  const audioOnlyRemotes = Math.max(0, (session.remoteAudioTracks ?? []).length - remoteVideos.length);

  const tileCount = 1 + (localScreenTrack ? 1 : 0) + remoteVideos.length + audioOnlyRemotes;

  return (
    <div className="call-grid" data-tiles={tileCount}>
      {/* Local camera / avatar */}
      <div className={`call-tile ${isSpeaking ? 'speaking' : ''}`}>
        {localCamTrack ? <VideoTile track={localCamTrack} mirror /> : <Avatar label={identity} />}
        <span className="tile-name">You</span>
      </div>

      {/* Local screen share */}
      {localScreenTrack && (
        <div className="call-tile screen">
          <VideoTile track={localScreenTrack} />
          <span className="tile-name">You — screen</span>
        </div>
      )}

      {/* Remote cameras */}
      {remoteVideos.map((v) => (
        <div key={v.id} className="call-tile">
          <VideoTile track={v.track} />
          <span className="tile-name">{shortName('Participant')}</span>
        </div>
      ))}

      {/* Audio-only remotes */}
      {Array.from({ length: audioOnlyRemotes }, (_, i) => (
        <div key={`audio-${i}`} className="call-tile">
          <Avatar label="P" />
          <span className="tile-name">Participant</span>
        </div>
      ))}
    </div>
  );
}
