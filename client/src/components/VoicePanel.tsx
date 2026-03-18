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
 *
 * The shared voice store is the single source of truth for:
 * - micMuted / deafened state (reflected by both StatusBar and in-call controls)
 * - input/output device IDs, volume levels
 * Device selection is applied to the LiveKit room when connected.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
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
import { useServersStore } from '@/stores/servers';
import * as api from '@/lib/api';
import { isTauri, getPlatformMediaStatus, setMediaKeepalive, type PlatformMediaStatus } from '@/lib/tauri';

// ── Helpers ──

/** Try to set the audio output device on an HTMLMediaElement (setSinkId).
 *  When `deviceId` is null/empty, resets to the system default ('').
 */
function trySetSinkId(el: HTMLMediaElement, deviceId: string | null): void {
  if (typeof (el as any).setSinkId === 'function') {
    (el as any).setSinkId(deviceId || '').catch(() => {});
  }
}

/** Apply current audio preferences (deafen, volume, output device) to a single audio element. */
function applyAudioPrefs(
  el: HTMLAudioElement,
  deafened: boolean,
  outputVolume: number,
  outputDeviceId: string | null,
): void {
  el.muted = deafened;
  el.volume = deafened ? 0 : Math.max(0, Math.min(1, outputVolume / 100));
  trySetSinkId(el, outputDeviceId);
}

/** Check whether setSinkId is supported in this browser/webview. */
function isSinkIdSupported(): boolean {
  return typeof HTMLMediaElement !== 'undefined' &&
    typeof (HTMLMediaElement.prototype as any).setSinkId === 'function';
}

/** Produce a user-friendly error message from a media toggle failure. */
function mediaErrorMessage(err: unknown, action: string): string {
  if (err instanceof DOMException) {
    if (err.name === 'NotAllowedError') {
      return `${action}: permission denied. Check your browser/OS settings.`;
    }
    if (err.name === 'NotFoundError') {
      return `${action}: no device found. Is your device connected?`;
    }
    if (err.name === 'AbortError' || err.name === 'NotReadableError') {
      return `${action}: device may be in use by another application.`;
    }
    return `${action}: ${err.message}`;
  }
  if (err instanceof Error) return `${action}: ${err.message}`;
  return `${action} failed`;
}

// ── Components ──

/** Local media status bar shown above the controls. */
function LocalMediaStatus() {
  const { isMicrophoneEnabled, isCameraEnabled, isScreenShareEnabled } = useLocalParticipant();

  return (
    <div className="local-media-status">
      <span className={`status-pill ${isMicrophoneEnabled ? 'on' : 'off'}`}>
        {isMicrophoneEnabled ? 'Mic ON' : 'Mic OFF'}
      </span>
      <span className={`status-pill ${isCameraEnabled ? 'on' : 'off'}`}>
        {isCameraEnabled ? 'Cam ON' : 'Cam OFF'}
      </span>
      {isScreenShareEnabled && (
        <span className="status-pill sharing">Sharing Screen</span>
      )}
    </div>
  );
}

/** Controls bar rendered inside the LiveKit room context. */
function MediaControls({
  onLeave,
  mediaStatus,
  platformWarnings,
}: {
  onLeave: () => void;
  mediaStatus: PlatformMediaStatus | null;
  platformWarnings: string[];
}) {
  const { localParticipant, isMicrophoneEnabled, isCameraEnabled, isScreenShareEnabled } = useLocalParticipant();
  const { micMuted, setMicMuted, deafened, cameraDeviceId } = useVoiceStore();

  const micEnabled = isMicrophoneEnabled;
  const camEnabled = isCameraEnabled;
  const screenEnabled = isScreenShareEnabled;

  // Platform capability checks — treat 'unknown' as available but with guidance
  const canScreenShare = mediaStatus?.screen_share_available !== false;
  const cameraMicStatus = mediaStatus?.camera_mic_available;
  const canCameraMic = cameraMicStatus !== false;
  const cameraMicUnknown = cameraMicStatus === 'unknown';

  // Error state for media toggle failures
  const [mediaError, setMediaError] = useState<string | null>(null);

  // Track whether a stale-camera confirmation is pending
  const [staleCameraPrompt, setStaleCameraPrompt] = useState(false);

  // Listen for device hot-plug events during an active call.
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

  // Sync voice store micMuted → LiveKit room state.
  // When micMuted changes in the store (from StatusBar or here), apply to LiveKit.
  useEffect(() => {
    const lp = localParticipant as LocalParticipant;
    if (!lp) return;
    const shouldBeEnabled = !micMuted;
    if (lp.isMicrophoneEnabled !== shouldBeEnabled) {
      lp.setMicrophoneEnabled(shouldBeEnabled).catch(() => {});
    }
  }, [micMuted, localParticipant]);

  // Sync LiveKit mic state → store when LiveKit state changes externally.
  useEffect(() => {
    if (micMuted !== !micEnabled) {
      setMicMuted(!micEnabled);
    }
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [micEnabled]);

  const toggleMic = useCallback(async () => {
    if (!canCameraMic) {
      setMediaError('Microphone is unavailable on this platform. Check your browser/OS privacy settings.');
      return;
    }
    try {
      setMediaError(null);
      await localParticipant.setMicrophoneEnabled(!micEnabled);
      setMicMuted(micEnabled); // toggling: if was enabled, now muted
    } catch (err) {
      setMediaError(mediaErrorMessage(err, micEnabled ? 'Mute microphone' : 'Unmute microphone'));
    }
  }, [localParticipant, micEnabled, canCameraMic, setMicMuted]);

  const toggleCamera = useCallback(async () => {
    if (!canCameraMic) {
      setMediaError('Camera is unavailable on this platform. Check your browser/OS privacy settings.');
      return;
    }
    try {
      setMediaError(null);
      if (!camEnabled && staleCameraPrompt) {
        // User confirmed fallback after stale camera error — use default camera
        setStaleCameraPrompt(false);
        await localParticipant.setCameraEnabled(true);
      } else if (!camEnabled && cameraDeviceId) {
        // Try to enable with the saved camera device
        try {
          await localParticipant.setCameraEnabled(true, { deviceId: cameraDeviceId });
        } catch (deviceErr) {
          // The selected camera may have been disconnected — surface an error
          // and ask the user to confirm fallback to default.
          if (deviceErr instanceof DOMException && (deviceErr.name === 'NotFoundError' || deviceErr.name === 'OverconstrainedError')) {
            setMediaError(`Saved camera not found. Click the camera button again to use the default camera, or change your camera in Audio Settings.`);
            setStaleCameraPrompt(true);
            return;
          }
          throw deviceErr;
        }
      } else {
        setStaleCameraPrompt(false);
        await localParticipant.setCameraEnabled(!camEnabled);
      }
    } catch (err) {
      setMediaError(mediaErrorMessage(err, camEnabled ? 'Turn off camera' : 'Turn on camera'));
    }
  }, [localParticipant, camEnabled, canCameraMic, cameraDeviceId, staleCameraPrompt]);

  const toggleScreen = useCallback(async () => {
    if (!canScreenShare && !screenEnabled) {
      const hint = platformWarnings.length > 0
        ? platformWarnings[0]
        : 'Screen sharing is unavailable on this platform.';
      setMediaError(hint);
      return;
    }
    try {
      setMediaError(null);
      await localParticipant.setScreenShareEnabled(!screenEnabled);
    } catch (err) {
      // Screen share cancelled by user is not an error
      if (err instanceof DOMException && err.name === 'AbortError') return;
      const hint = platformWarnings.length > 0
        ? `Screen sharing failed. ${platformWarnings[0]}`
        : mediaErrorMessage(err, screenEnabled ? 'Stop sharing' : 'Share screen');
      setMediaError(hint);
    }
  }, [localParticipant, screenEnabled, canScreenShare, platformWarnings]);

  const screenShareTitle = !canScreenShare
    ? 'Screen sharing is unavailable on this platform'
    : screenEnabled
      ? 'Stop sharing'
      : 'Share screen';

  const cameraMicTitle = !canCameraMic
    ? 'Camera/microphone unavailable on this platform'
    : cameraMicUnknown
      ? 'Camera/microphone permission could not be verified — may require manual grant'
      : undefined;

  return (
    <div className="media-controls">
      {deviceNotice && (
        <div className="device-notice" role="status">{deviceNotice}</div>
      )}
      {mediaError && (
        <div className="media-error" role="alert">
          <span>{mediaError}</span>
          <button
            onClick={() => setMediaError(null)}
            className="media-error-dismiss"
            aria-label="Dismiss"
          >
            &times;
          </button>
        </div>
      )}
      {cameraMicUnknown && (
        <div className="device-notice" role="status">
          Camera/microphone permission status unknown — you may need to grant access in your OS settings.
        </div>
      )}
      {deafened && (
        <div className="deafen-notice" role="status">
          You are deafened — all incoming audio is muted.
        </div>
      )}
      <button
        className={`media-control-btn ${micEnabled ? 'active' : 'muted'}`}
        onClick={toggleMic}
        disabled={!canCameraMic}
        title={cameraMicTitle ?? (micEnabled ? 'Mute microphone' : 'Unmute microphone')}
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
        disabled={!canCameraMic}
        title={cameraMicTitle ?? (camEnabled ? 'Turn off camera' : 'Turn on camera')}
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
        className={`media-control-btn screen-btn ${screenEnabled ? 'active sharing' : ''} ${!canScreenShare ? 'disabled-cap' : ''}`}
        onClick={toggleScreen}
        disabled={!canScreenShare && !screenEnabled}
        title={screenShareTitle}
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

/**
 * Re-enable media tracks killed by the OS/webview when the Tauri window
 * loses focus or is minimized.
 *
 * **Layer 1** (Rust-side `set_media_keepalive`) prevents most track deaths
 * by keeping `ICoreWebView2Controller::IsVisible = true` during calls.
 * This hook is **Layer 2**: a safety net that detects and recovers any
 * tracks that still ended despite the keepalive.
 *
 * For screen share, we first attempt a silent auto-restart. In WebView2,
 * `getDisplayMedia()` may succeed programmatically because the permission
 * model is more relaxed than Chrome. If that fails (user gesture required),
 * `onScreenShareInterrupted` fires so the UI can show a resume banner
 * (**Layer 3**).
 *
 * We listen to BOTH `visibilitychange` (fires on minimize) and `window.focus`
 * (fires on alt-tab back) because a simple alt-tab may not trigger
 * `visibilitychange` on WebView2 — the document can stay "visible" while
 * unfocused, yet the OS still kills the MediaStreamTracks.
 */
function useTauriMediaRestore(onScreenShareInterrupted?: () => void) {
  const { localParticipant } = useLocalParticipant();
  const cameraDeviceId = useVoiceStore((s) => s.cameraDeviceId);

  useEffect(() => {
    if (!isTauri()) return;

    let restoring = false;

    const restoreMedia = async () => {
      // Guard against visibilitychange and focus both firing in quick succession
      if (restoring) return;
      // Skip if the document is still hidden (the 'hidden' transition of visibilitychange)
      if (document.visibilityState === 'hidden') return;

      restoring = true;
      try {
        // Brief delay for the webview to fully resume
        await new Promise((r) => setTimeout(r, 200));

        const lp = localParticipant as LocalParticipant;

        // Helper: find a publication by source from the participant's track map.
        const findPub = (source: Track.Source) => {
          for (const pub of lp.trackPublications.values()) {
            if (pub.source === source) return pub;
          }
          return undefined;
        };

        // Re-enable mic if it was on but the track ended
        if (lp.isMicrophoneEnabled) {
          const pub = findPub(Track.Source.Microphone);
          if (pub?.track?.mediaStreamTrack?.readyState === 'ended') {
            try {
              await lp.setMicrophoneEnabled(false);
              await lp.setMicrophoneEnabled(true);
            } catch { /* best effort */ }
          }
        }

        // Re-enable camera if it was on but the track ended
        if (lp.isCameraEnabled) {
          const pub = findPub(Track.Source.Camera);
          if (pub?.track?.mediaStreamTrack?.readyState === 'ended') {
            try {
              await lp.setCameraEnabled(false);
              const camOpts = cameraDeviceId ? { deviceId: cameraDeviceId } : undefined;
              await lp.setCameraEnabled(true, camOpts);
            } catch { /* best effort */ }
          }
        }

        // Screen share: attempt silent auto-restart first. WebView2 may
        // allow getDisplayMedia() without a user gesture (unlike Chrome).
        // If that fails, clean up and notify the UI to show a resume banner.
        if (lp.isScreenShareEnabled) {
          const pub = findPub(Track.Source.ScreenShare);
          if (pub?.track?.mediaStreamTrack?.readyState === 'ended') {
            try {
              await lp.setScreenShareEnabled(false);
              await lp.setScreenShareEnabled(true);
            } catch {
              // getDisplayMedia failed (gesture required) — clean up and notify
              try { await lp.setScreenShareEnabled(false); } catch { /* ignore */ }
              onScreenShareInterrupted?.();
            }
          }
        }
      } finally {
        restoring = false;
      }
    };

    document.addEventListener('visibilitychange', restoreMedia);
    window.addEventListener('focus', restoreMedia);
    return () => {
      document.removeEventListener('visibilitychange', restoreMedia);
      window.removeEventListener('focus', restoreMedia);
    };
  }, [localParticipant, onScreenShareInterrupted, cameraDeviceId]);
}

/**
 * Apply voice store settings to the active LiveKit room:
 * - Input device selection via media constraints
 * - Output device via setSinkId on audio elements
 * - Output volume on audio elements
 * - Deafen by muting all remote audio elements
 */
function useVoiceStoreSync() {
  const { localParticipant } = useLocalParticipant();
  const { inputDeviceId, outputDeviceId, outputVolume, deafened, cameraDeviceId } = useVoiceStore();

  // Apply input device selection when it changes (including reset to System Default)
  useEffect(() => {
    const lp = localParticipant as LocalParticipant;
    if (!lp.isMicrophoneEnabled) return;
    // Re-publish microphone with the selected device, or default constraints
    const opts = inputDeviceId ? { deviceId: inputDeviceId } : undefined;
    lp.setMicrophoneEnabled(false)
      .then(() => lp.setMicrophoneEnabled(true, opts))
      .catch((err) => { console.warn('[VoicePanel] mic device switch failed:', err); });
  // Only run when inputDeviceId changes, not on every mic toggle
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inputDeviceId]);

  // When cameraDeviceId changes during an active call and camera is already on,
  // republish the camera track with the new device (or default constraints).
  useEffect(() => {
    const lp = localParticipant as LocalParticipant;
    if (!lp.isCameraEnabled) return;
    const opts = cameraDeviceId ? { deviceId: cameraDeviceId } : undefined;
    lp.setCameraEnabled(false)
      .then(() => lp.setCameraEnabled(true, opts))
      .catch((err) => { console.warn('[VoicePanel] camera device switch failed:', err); });
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cameraDeviceId]);

  // Apply output device and volume to all <audio> elements rendered by RoomAudioRenderer.
  // Also handles deafen by muting those elements.
  // When outputDeviceId is null/empty, reset to system default via setSinkId('').
  useEffect(() => {
    const audioElements = document.querySelectorAll<HTMLAudioElement>('audio[data-lk-source]');
    audioElements.forEach((el) => {
      applyAudioPrefs(el, deafened, outputVolume, outputDeviceId);
    });

    // Also handle any audio elements inside livekit containers that may not have data-lk-source
    const lkAudioElements = document.querySelectorAll<HTMLAudioElement>('[data-testid="livekit-room"] audio, .lk-room-container audio');
    lkAudioElements.forEach((el) => {
      applyAudioPrefs(el, deafened, outputVolume, outputDeviceId);
    });
  }, [deafened, outputVolume, outputDeviceId]);

  // Set up a MutationObserver to catch dynamically added audio elements.
  // Handles both direct <audio> nodes and container nodes with <audio> descendants.
  // Capture current values in a ref so the observer callback always uses fresh state.
  const deafenedRef = useRef(deafened);
  const outputVolumeRef = useRef(outputVolume);
  const outputDeviceIdRef = useRef(outputDeviceId);
  deafenedRef.current = deafened;
  outputVolumeRef.current = outputVolume;
  outputDeviceIdRef.current = outputDeviceId;

  useEffect(() => {
    const applyToAudio = (el: HTMLAudioElement) => {
      applyAudioPrefs(el, deafenedRef.current, outputVolumeRef.current, outputDeviceIdRef.current);
    };

    const observer = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
        for (const node of mutation.addedNodes) {
          if (node instanceof HTMLAudioElement) {
            applyToAudio(node);
          } else if (node instanceof HTMLElement) {
            // Scan descendants for <audio> elements inside container nodes
            const nested = node.querySelectorAll<HTMLAudioElement>('audio');
            nested.forEach(applyToAudio);
          }
        }
      }
    });

    observer.observe(document.body, { childList: true, subtree: true });
    return () => observer.disconnect();
  }, []);
}

/** Room content rendered inside the LiveKitRoom context. */
function RoomContent({
  onLeave,
  mediaStatus,
  platformWarnings,
}: {
  onLeave: () => void;
  mediaStatus: PlatformMediaStatus | null;
  platformWarnings: string[];
}) {
  const { localParticipant, isScreenShareEnabled } = useLocalParticipant();
  const [screenShareInterrupted, setScreenShareInterrupted] = useState(false);

  // Layer 1: Tell the Rust backend to keep the webview alive during the call.
  // This prevents WebView2 from setting IsVisible=false on minimize, which
  // would kill MediaStreamTracks.
  useEffect(() => {
    if (!isTauri()) return;
    setMediaKeepalive(true).catch(() => {});
    return () => { setMediaKeepalive(false).catch(() => {}); };
  }, []);

  // Layer 2: Safety-net hook that restores any tracks that still died.
  // If screen share can't auto-restart, it fires the callback for Layer 3.
  const handleScreenShareInterrupted = useCallback(() => {
    setScreenShareInterrupted(true);
  }, []);
  useTauriMediaRestore(handleScreenShareInterrupted);

  // Sync voice store settings (device selection, volume, deafen) to the room.
  useVoiceStoreSync();

  // Clear the interrupted banner when the user manually re-enables screen share.
  useEffect(() => {
    if (isScreenShareEnabled) setScreenShareInterrupted(false);
  }, [isScreenShareEnabled]);

  // Layer 3: Resume banner — the button click provides the user gesture
  // needed for getDisplayMedia() in browsers that require it.
  const resumeScreenShare = useCallback(async () => {
    setScreenShareInterrupted(false);
    try {
      await (localParticipant as LocalParticipant).setScreenShareEnabled(true);
    } catch { /* user cancelled the picker or error — stay dismissed */ }
  }, [localParticipant]);

  return (
    <>
      <RoomAudioRenderer />
      {screenShareInterrupted && (
        <div className="screen-share-interrupted" role="alert">
          <span>Screen share was interrupted</span>
          <button onClick={resumeScreenShare} className="screen-share-resume-btn">
            Resume Sharing
          </button>
          <button
            onClick={() => setScreenShareInterrupted(false)}
            className="screen-share-dismiss-btn"
            aria-label="Dismiss"
          >
            &times;
          </button>
        </div>
      )}
      <LocalMediaStatus />
      <LocalSelfView />
      <ScreenShareView />
      <ParticipantGrid />
      <MediaControls
        onLeave={onLeave}
        mediaStatus={mediaStatus}
        platformWarnings={platformWarnings}
      />
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
  const activeServerId = useServersStore((s) => s.activeServerId);

  const {
    voiceToken,
    livekitUrl,
    iceServers,
    connectedChannelId,
    joining,
    joinCall,
    leaveCall,
    checkCallActive,
    isCallActive,
    getJoinError,
    clearChannelCallState,
  } = useVoiceStore();

  // Track the server ID that was active when the call was joined.
  // Capture only on session establishment (transition from no token to active token),
  // not on every activeServerId update, to prevent a later server switch from
  // overwriting the stored value.
  const callServerIdRef = useRef<string | null>(null);
  const prevTokenRef = useRef<string | null>(null);
  useEffect(() => {
    const hadToken = !!prevTokenRef.current;
    const hasToken = !!voiceToken;
    prevTokenRef.current = voiceToken;
    // Only set the ref when a new session starts (no token → token)
    if (!hadToken && hasToken && connectedChannelId) {
      callServerIdRef.current = activeServerId;
    }
    // Clear when the session ends
    if (!hasToken) {
      callServerIdRef.current = null;
    }
  }, [voiceToken, connectedChannelId, activeServerId]);

  // Guard: if the active server changed since we joined, the token is stale.
  const isTokenStale = !!(
    voiceToken &&
    connectedChannelId &&
    callServerIdRef.current !== null &&
    callServerIdRef.current !== activeServerId
  );

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

  const platformWarnings = mediaStatus?.warnings ?? [];

  const activeChannel = channels.find((c) => c.channel_id === activeChannelId);
  const isVoiceCapable =
    activeChannel?.channel_type === 'Voice' || activeChannel?.channel_type === 'Hybrid';
  const isVoiceAllowed = permissions?.capabilities.can_voice ?? true;
  const canJoinVoice = isVoiceAllowed;

  // Clear stale call state when switching away from a channel so the next
  // channel does not inherit the previous channel's status or errors.
  const prevChannelRef = useRef<string | null>(null);
  useEffect(() => {
    const prev = prevChannelRef.current;
    prevChannelRef.current = activeChannelId;
    if (prev && prev !== activeChannelId) {
      clearChannelCallState(prev);
    }
  }, [activeChannelId, clearChannelCallState]);

  // Derive per-channel call-active status and join error.
  const callActive = activeChannelId ? isCallActive(activeChannelId) : false;
  const lastJoinErrorDetails = activeChannelId ? getJoinError(activeChannelId) : null;
  const lastJoinError = lastJoinErrorDetails?.display ?? null;

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

  // Use setup hint from structured error, or fetch from server as fallback.
  const [setupHint, setSetupHint] = useState<string | null>(null);
  useEffect(() => {
    // Prefer the structured setup_hint from the join response
    if (lastJoinErrorDetails?.setupHint) {
      setSetupHint(lastJoinErrorDetails.setupHint);
      return;
    }

    const isVoiceNotConfigured =
      lastJoinErrorDetails?.code === 'voice_not_configured' ||
      lastJoinError?.includes('not configured');

    if (!isVoiceNotConfigured) {
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
  }, [lastJoinError, lastJoinErrorDetails]);

  // Prevent LiveKit from disconnecting when the Tauri webview fires page-leave events.
  const roomOptions = useMemo(() => {
    if (!isTauri()) return undefined;
    return { disconnectOnPageLeave: false };
  }, []);

  // Build RTC configuration with server-provided ICE servers for NAT traversal.
  const connectOptions = useMemo(() => {
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
  // But NOT if the token is stale from a server switch.
  if (voiceToken && livekitUrl && connectedChannelId && !isTokenStale) {
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
          connectOptions={connectOptions}
        >
          <RoomContent
            onLeave={handleLeave}
            mediaStatus={mediaStatus}
            platformWarnings={platformWarnings}
          />
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
          {lastJoinError && <p>{lastJoinError}</p>}
          {!lastJoinError && unavailableReason && <p>{unavailableReason}</p>}
          {setupHint && <p className="voice-setup-hint">{setupHint}</p>}
        </div>
      )}
    </div>
  );
}
