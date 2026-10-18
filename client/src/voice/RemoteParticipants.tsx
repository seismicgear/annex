/**
 * Participant visualisation for an in-progress call.
 *
 * A single responsive grid of equal tiles: your own camera (or avatar), your
 * screen share if active, one tile per remote camera the SFU forwards, and an
 * avatar tile for any audio-only remote. Each tile shows the name, a speaking
 * ring, and a camera-off avatar fallback — so the call looks like a real
 * conferencing grid rather than stacked green boxes.
 */

import { useEffect, useRef, useState } from 'react';
import { TrackSource, type WebRtcSession } from '@/lib/webrtc';
import { useVoiceStore } from '@/stores/voice';
import { useUsernameStore } from '@/stores/usernames';

/** Renders a MediaStreamTrack into a <video>. `mirror` flips the local camera. */
function VideoTile({ track, publication, mirror = false }: {
  track: MediaStreamTrack;
  /** A fresh ontrack publication can reuse the same browser receiver track. */
  publication?: object;
  mirror?: boolean;
}) {
  const ref = useRef<HTMLVideoElement>(null);
  const resumeRef = useRef<() => void>(() => {});
  const [playbackBlocked, setPlaybackBlocked] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    let active = true;
    let attempt = 0;
    const play = () => {
      if (!active || track.readyState !== 'live' || track.muted) return;
      const currentAttempt = ++attempt;
      void el.play()?.then(() => {
        if (active && currentAttempt === attempt) setPlaybackBlocked(false);
      }).catch(() => {
        if (active && currentAttempt === attempt) setPlaybackBlocked(true);
      });
    };
    const attach = () => {
      // An inactive SFU transceiver can resume with the same track object.
      // Reattach a fresh stream and explicitly resume the media element;
      // autoplay alone can leave the previous, exhausted stream paused.
      el.srcObject = new MediaStream([track]);
      play();
    };
    resumeRef.current = play;
    track.addEventListener('unmute', attach);
    el.addEventListener('canplay', play);
    attach();
    return () => {
      active = false;
      resumeRef.current = () => {};
      track.removeEventListener('unmute', attach);
      el.removeEventListener('canplay', play);
      el.srcObject = null;
    };
  }, [track, publication]);
  return (
    <>
      <video
        ref={ref}
        autoPlay
        playsInline
        muted
        className="tile-video"
        style={mirror ? { transform: 'scaleX(-1)' } : undefined}
      />
      {playbackBlocked && (
        <button className="tile-video-resume" onClick={() => resumeRef.current()}>Resume video</button>
      )}
    </>
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

/**
 * Unified participant grid: local camera/avatar, local screen share, every
 * remote camera forwarded by the SFU, plus avatar tiles for audio-only remotes.
 */
export function ParticipantGrid({ session }: { session: WebRtcSession }) {
  const identity = session.identity;
  const isSpeaking = session.isSpeaking;

  // Who else is in this call, from the server's roster. The SFU has always
  // keyed peers by pseudonym; until now only the count was exposed, so every
  // remote tile rendered the literal string "Participant" and a call of four
  // looked like four identical anonymous boxes.
  const connectedChannelId = useVoiceStore((s) => s.connectedChannelId);
  // `participantsByChannel` is defaulted rather than indexed directly: a store
  // rehydrated from a build that predates it has no such key, and a missing
  // roster must degrade to unnamed tiles, not crash the whole call view.
  const roster = useVoiceStore((s) =>
    connectedChannelId ? ((s.participantsByChannel ?? {})[connectedChannelId] ?? []) : [],
  );
  const getDisplayName = useUsernameStore((s) => s.getDisplayName);
  const others = roster.filter((id) => id !== identity);
  const nameFor = (id: string) => getDisplayName(id) ?? shortName(id);

  const camPub = session.trackPublications.get(TrackSource.Camera);
  const screenPub = session.trackPublications.get(TrackSource.ScreenShare);
  const localCamTrack = camPub && !camPub.isMuted && camPub.track ? camPub.track.mediaStreamTrack : null;
  const localScreenTrack = screenPub && !screenPub.isMuted && screenPub.track ? screenPub.track.mediaStreamTrack : null;

  // Remote cameras the SFU forwards. The SFU pre-creates a video slot per peer
  // that is muted until that peer publishes — only show tiles that are live.
  const remoteVideos = (session.remoteVideoTracks ?? []).filter(
    (v) => v.isPublished !== false && v.track.readyState === 'live' && !v.track.muted,
  );
  // Audio-only remotes.
  //
  // NOT derived from `remoteAudioTracks.length`. The SFU attaches three
  // outbound tracks to every peer connection at join time — an audio mix, a
  // video slot, and the agent TTS track — so a user sitting alone in a channel
  // already has inbound audio tracks and was shown a tile captioned
  // "Participant". Joining an empty voice channel looked exactly like joining
  // one that somebody else was already in.
  //
  // The server roster knows who is actually in the room, so count from that
  // and let the tracks decide only which of them are sending video.
  // Who is in the call but not on screen: everyone in the roster who has no
  // live video track of their own. Each remote sender now has its own track
  // (see `senderFromTrackId`), so this is a set difference rather than a count
  // — with per-sender attribution there is no need to guess.
  const sendingVideo = new Set(
    remoteVideos.map((v) => v.sender).filter((s): s is string => s !== null),
  );
  const audioOnly = others.filter((id) => !sendingVideo.has(id));

  const tileCount = 1 + (localScreenTrack ? 1 : 0) + remoteVideos.length + audioOnly.length;

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

      {/* Camera and screen publications have separate, attributable tracks. */}
      {remoteVideos.map((v) => (
        <div key={v.id} className={`call-tile${v.id.startsWith('screen-') ? ' screen' : ''}`}>
          <VideoTile track={v.track} publication={v} />
          <span className="tile-name">
            {v.sender ? nameFor(v.sender) : 'Participant'}
            {v.id.startsWith('screen-') ? ' — screen' : ''}
          </span>
        </div>
      ))}

      {/* Audio-only remotes: in the call but not sending video. Derived from
          the roster rather than the track list, because the SFU attaches
          tracks to a connection before the peer is necessarily publishing. */}
      {audioOnly.map((id) => (
        <div key={`audio-${id}`} className="call-tile">
          <Avatar label={nameFor(id)} />
          <span className="tile-name">{nameFor(id)}</span>
        </div>
      ))}
    </div>
  );
}
