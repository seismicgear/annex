/**
 * Participant visualisation: the local self-view tiles for camera and
 * screen share, the participant grid (local tile + one tile per remote
 * audio track), and the placeholder remote screen-share view.
 *
 * The SFU is currently audio-first and does not forward video tracks, so
 * remote tiles render an avatar circle rather than a video element.
 */

import { useEffect, useRef } from 'react';
import { TrackSource, type WebRtcSession } from '@/lib/webrtc';

/** Native video track renderer. */
function NativeVideoTrack({ track, muted = true }: { track: MediaStreamTrack; muted?: boolean }) {
  const videoRef = useRef<HTMLVideoElement>(null);

  useEffect(() => {
    const el = videoRef.current;
    if (!el) return;
    el.srcObject = new MediaStream([track]);
    return () => { el.srcObject = null; };
  }, [track]);

  return <video ref={videoRef} autoPlay playsInline muted={muted} style={{ width: '100%', height: '100%', objectFit: 'cover' }} />;
}

/** Local self-view: shows your own camera and screen share. */
export function LocalSelfView({ session }: { session: WebRtcSession }) {
  const camPub = session.trackPublications.get(TrackSource.Camera);
  const screenPub = session.trackPublications.get(TrackSource.ScreenShare);

  const localCamTrack = camPub && !camPub.isMuted && camPub.track ? camPub.track.mediaStreamTrack : null;
  const localScreenTrack = screenPub && !screenPub.isMuted && screenPub.track ? screenPub.track.mediaStreamTrack : null;

  if (!localCamTrack && !localScreenTrack) return null;

  return (
    <div className="local-self-view">
      {localCamTrack && (
        <div className="self-view-tile">
          <NativeVideoTrack track={localCamTrack} />
          <span className="self-view-label">You (camera)</span>
        </div>
      )}
      {localScreenTrack && (
        <div className="self-view-tile screen">
          <NativeVideoTrack track={localScreenTrack} />
          <span className="self-view-label">You (screen)</span>
        </div>
      )}
    </div>
  );
}

/** Prominent screen share display when someone else is sharing.
 *  The SFU is audio-first and does not forward video tracks yet,
 *  so remote screen shares are not available. This component is
 *  retained for forward compatibility when video forwarding is added.
 */
export function ScreenShareView() {
  // No remote video tracks from the SFU currently — return null.
  return null;
}

/** Participant grid — shows local user tile with camera/speaking state.
 *  Remote participants are represented by their incoming audio tracks.
 *  The SFU does not broadcast a participant roster, so remote entries
 *  are derived from the number of remote audio tracks received.
 */
export function ParticipantGrid({ session }: { session: WebRtcSession }) {
  const identity = session.identity;
  const isSpeaking = session.isSpeaking;
  const camPub = session.trackPublications.get(TrackSource.Camera);
  const localCamTrack = camPub && !camPub.isMuted && camPub.track ? camPub.track.mediaStreamTrack : null;
  const hasAnyVideo = !!localCamTrack;
  const remoteTrackCount = session.remoteAudioTracks.length;

  return (
    <div className={`participant-grid ${hasAnyVideo ? 'has-video' : 'audio-only'}`}>
      {/* Local participant */}
      {localCamTrack ? (
        <div className={`participant-tile video ${isSpeaking ? 'speaking' : ''}`}>
          <NativeVideoTrack track={localCamTrack} />
          <span className="participant-label">
            {identity.slice(0, 12)}...
            {isSpeaking && <span className="speaking-indicator" />}
          </span>
        </div>
      ) : (
        <div className={`participant-tile audio-tile ${isSpeaking ? 'speaking' : ''}`}>
          <div className="participant-avatar-circle">
            {identity.charAt(0).toUpperCase()}
          </div>
          <span className="participant-label">
            {identity.slice(0, 12)}...
            {isSpeaking && <span className="speaking-indicator" />}
          </span>
        </div>
      )}
      {/* Remote participants — one tile per incoming audio track */}
      {Array.from({ length: remoteTrackCount }, (_, i) => (
        <div key={`remote-${i}`} className="participant-tile audio-tile">
          <div className="participant-avatar-circle">?</div>
          <span className="participant-label">Participant</span>
        </div>
      ))}
    </div>
  );
}
