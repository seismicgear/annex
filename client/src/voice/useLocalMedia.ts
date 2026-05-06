/**
 * Local-media controls bag for MediaControls.
 *
 * Owns the toggle handlers (mic, camera, screen share), surfaces the
 * platform-capability flags + matching titles, runs the bidirectional
 * micMuted ↔ session sync, the input/camera device-change re-publish
 * effects, and a `devicechange` listener that flashes the user a notice.
 *
 * Also exports `useTauriMediaRestore` — a separate hook driven by
 * RoomContent so that it can wire its `onScreenShareInterrupted` callback
 * into the Layer-3 "Resume Sharing" banner UI.
 */

import { useCallback, useEffect, useRef, useState } from 'react';
import { TrackSource, type WebRtcSession } from '@/lib/webrtc';
import { isTauri, type PlatformMediaStatus } from '@/lib/tauri';
import { useVoiceStore } from '@/stores/voice';

/** Produce a user-friendly error message from a media toggle failure. */
export function mediaErrorMessage(err: unknown, action: string): string {
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

interface UseLocalMediaArgs {
  localParticipant: WebRtcSession;
  isMicrophoneEnabled: boolean;
  isCameraEnabled: boolean;
  isScreenShareEnabled: boolean;
  mediaStatus: PlatformMediaStatus | null;
  platformWarnings: string[];
}

interface UseLocalMediaResult {
  micEnabled: boolean;
  camEnabled: boolean;
  screenEnabled: boolean;
  canScreenShare: boolean;
  canCameraMic: boolean;
  cameraMicBlocked: boolean;
  cameraMicUnknown: boolean;
  isMacScreenShareUnknown: boolean;
  deafened: boolean;
  mediaError: string | null;
  setMediaError: (message: string | null) => void;
  staleCameraPrompt: boolean;
  deviceNotice: string | null;
  toggleMic: () => Promise<void>;
  toggleCamera: () => Promise<void>;
  toggleScreen: () => Promise<void>;
  handleCameraFallback: () => Promise<void>;
  screenShareTitle: string;
  cameraMicTitle: string | undefined;
}

export function useLocalMedia({
  localParticipant,
  isMicrophoneEnabled,
  isCameraEnabled,
  isScreenShareEnabled,
  mediaStatus,
  platformWarnings,
}: UseLocalMediaArgs): UseLocalMediaResult {
  const {
    micMuted,
    setMicMuted,
    deafened,
    cameraDeviceId,
    setCameraDevice,
    inputDeviceId,
    setInputDevice,
  } = useVoiceStore();

  const micEnabled = isMicrophoneEnabled;
  const camEnabled = isCameraEnabled;
  const screenEnabled = isScreenShareEnabled;

  // Platform capability checks — treat 'unknown' as available but with guidance;
  // treat 'blocked' as unavailable (same as false).
  const canScreenShare = mediaStatus?.screen_share_available !== false && mediaStatus?.screen_share_available !== 'blocked';
  const cameraMicStatus = mediaStatus?.camera_mic_available;
  const canCameraMic = cameraMicStatus !== false && cameraMicStatus !== 'blocked';
  const cameraMicBlocked = cameraMicStatus === 'blocked';
  const cameraMicUnknown = cameraMicStatus === 'unknown';

  // Error state for media toggle failures
  const [mediaError, setMediaError] = useState<string | null>(null);

  // Track whether a stale-camera confirmation is pending
  const [staleCameraPrompt, setStaleCameraPrompt] = useState(false);

  // Listen for device hot-plug events during an active call.
  const [deviceNotice, setDeviceNotice] = useState<string | null>(null);
  useEffect(() => {
    if (!navigator.mediaDevices?.addEventListener) return;
    let noticeTimer: ReturnType<typeof setTimeout> | null = null;
    const handler = () => {
      setDeviceNotice('Audio/video device changed. Open Audio Settings to select.');
      if (noticeTimer) clearTimeout(noticeTimer);
      noticeTimer = setTimeout(() => setDeviceNotice(null), 5000);
    };
    navigator.mediaDevices.addEventListener('devicechange', handler);
    return () => {
      navigator.mediaDevices.removeEventListener('devicechange', handler);
      if (noticeTimer) clearTimeout(noticeTimer);
    };
  }, []);

  // Sync voice store micMuted → WebRTC room state.
  // When micMuted changes in the store (from StatusBar or here), apply to WebRTC.
  // On failure, revert the store to match the real WebRTC participant state.
  useEffect(() => {
    const lp = localParticipant;
    if (!lp) return;
    const shouldBeEnabled = !micMuted;
    if (lp.isMicrophoneEnabled !== shouldBeEnabled) {
      const priorEnabled = lp.isMicrophoneEnabled;
      lp.setMicrophoneEnabled(shouldBeEnabled).catch((err) => {
        console.warn('[VoicePanel] mic sync failed:', err);
        // Revert store to match the real WebRTC microphone state
        setMicMuted(!priorEnabled);
        setMediaError(mediaErrorMessage(err, 'Microphone toggle'));
      });
    }
  }, [micMuted, localParticipant, setMicMuted]);

  // Sync WebRTC mic state → store when WebRTC state changes externally.
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
      if (micEnabled) {
        // Muting — no device options needed
        await localParticipant.setMicrophoneEnabled(false);
      } else {
        // Enabling — pass the stored input device so first-time enable
        // and post-mute unmute both respect the selected device.
        const opts = inputDeviceId ? { deviceId: inputDeviceId } : undefined;
        await localParticipant.setMicrophoneEnabled(true, opts);
      }
      setMicMuted(micEnabled); // toggling: if was enabled, now muted
    } catch (err) {
      setMediaError(mediaErrorMessage(err, micEnabled ? 'Mute microphone' : 'Unmute microphone'));
    }
  }, [localParticipant, micEnabled, canCameraMic, setMicMuted, inputDeviceId]);

  const toggleCamera = useCallback(async () => {
    if (!canCameraMic) {
      setMediaError('Camera is unavailable on this platform. Check your browser/OS privacy settings.');
      return;
    }
    try {
      setMediaError(null);
      if (!camEnabled && cameraDeviceId) {
        // Try to enable with the saved camera device
        try {
          await localParticipant.setCameraEnabled(true, { deviceId: cameraDeviceId });
          setStaleCameraPrompt(false);
        } catch (deviceErr) {
          // The selected camera may have been disconnected — show inline recovery UI
          if (deviceErr instanceof DOMException && (deviceErr.name === 'NotFoundError' || deviceErr.name === 'OverconstrainedError')) {
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
  }, [localParticipant, camEnabled, canCameraMic, cameraDeviceId]);

  // Recovery action: clear saved device and retry with default camera
  const handleCameraFallback = useCallback(async () => {
    setStaleCameraPrompt(false);
    setCameraDevice(null);
    try {
      await localParticipant.setCameraEnabled(true);
    } catch (err) {
      setMediaError(mediaErrorMessage(err, 'Turn on camera'));
    }
  }, [localParticipant, setCameraDevice]);

  // Clear stale camera state when camera starts, device changes, or call ends
  useEffect(() => {
    if (camEnabled) setStaleCameraPrompt(false);
  }, [camEnabled]);

  // Determine macOS-specific screen share readiness
  const screenShareStatus = mediaStatus?.screen_share_available;
  const isMacScreenShareUnknown = mediaStatus?.display_server === 'macos' && screenShareStatus !== false;

  const toggleScreen = useCallback(async () => {
    if (!canScreenShare && !screenEnabled) {
      // macOS: provide targeted Screen Recording guidance
      if (mediaStatus?.display_server === 'macos') {
        setMediaError(
          'Screen sharing requires Screen Recording permission. ' +
          'Enable it in System Settings → Privacy & Security → Screen Recording, then restart the app.',
        );
      } else {
        const hint = platformWarnings.length > 0
          ? platformWarnings[0]
          : 'Screen sharing is unavailable on this platform.';
        setMediaError(hint);
      }
      return;
    }
    try {
      setMediaError(null);
      await localParticipant.setScreenShareEnabled(!screenEnabled);
    } catch (err) {
      // Distinguish user-cancel from real AbortError failures
      if (err instanceof DOMException && err.name === 'AbortError') {
        // Check if this looks like a user cancellation vs a runtime failure.
        // User cancels from the screen picker typically have a generic message.
        const isLikelyUserCancel = !err.message || /user/i.test(err.message) || err.message === 'AbortError';
        if (isLikelyUserCancel) return;
        // Runtime AbortError — surface with platform guidance
      }
      // macOS: map failures to Screen Recording message
      if (mediaStatus?.display_server === 'macos') {
        setMediaError(
          'Screen sharing failed — Screen Recording permission may be required. ' +
          'Enable it in System Settings → Privacy & Security → Screen Recording, then restart the app.',
        );
      } else {
        const hint = platformWarnings.length > 0
          ? `Screen sharing failed. ${platformWarnings[0]}`
          : mediaErrorMessage(err, screenEnabled ? 'Stop sharing' : 'Share screen');
        setMediaError(hint);
      }
    }
  }, [localParticipant, screenEnabled, canScreenShare, platformWarnings, mediaStatus]);

  const screenShareTitle = !canScreenShare
    ? 'Screen sharing is unavailable on this platform'
    : isMacScreenShareUnknown && !screenEnabled
      ? 'Share screen (Screen Recording permission may be required)'
      : screenEnabled
        ? 'Stop sharing'
        : 'Share screen';

  const cameraMicTitle = cameraMicBlocked
    ? 'Camera/microphone blocked — check your OS privacy settings to allow access'
    : !canCameraMic
      ? 'Camera/microphone unavailable on this platform'
      : cameraMicUnknown
        ? 'Camera/microphone permission could not be verified — may require manual grant'
        : undefined;

  // Apply input device selection when it changes (including reset to System Default).
  // On failure, roll back the store to the previous device ID and surface the error.
  const prevInputDeviceRef = useRef(inputDeviceId);
  useEffect(() => {
    const prevDeviceId = prevInputDeviceRef.current;
    prevInputDeviceRef.current = inputDeviceId;

    const lp = localParticipant;
    if (!lp.isMicrophoneEnabled) return;
    // Re-publish microphone with the selected device, or default constraints
    const opts = inputDeviceId ? { deviceId: inputDeviceId } : undefined;
    lp.setMicrophoneEnabled(false)
      .then(() => lp.setMicrophoneEnabled(true, opts))
      .catch((err) => {
        console.warn('[VoicePanel] mic device switch failed:', err);
        // Roll back to the previous working device ID
        setInputDevice(prevDeviceId);
        useVoiceStore.getState().setMicMuted(!lp.isMicrophoneEnabled);
        useVoiceStore.setState({
          micToggleError: mediaErrorMessage(err, 'Microphone device switch'),
        });
      });
  // Only run when inputDeviceId changes, not on every mic toggle
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inputDeviceId]);

  // When cameraDeviceId changes during an active call and camera is already on,
  // republish the camera track with the new device (or default constraints).
  // On failure, roll back and surface the error.
  const prevCameraDeviceRef = useRef(cameraDeviceId);
  useEffect(() => {
    const prevDeviceId = prevCameraDeviceRef.current;
    prevCameraDeviceRef.current = cameraDeviceId;

    const lp = localParticipant;
    if (!lp.isCameraEnabled) return;
    const opts = cameraDeviceId ? { deviceId: cameraDeviceId } : undefined;
    lp.setCameraEnabled(false)
      .then(() => lp.setCameraEnabled(true, opts))
      .catch((err) => {
        console.warn('[VoicePanel] camera device switch failed:', err);
        // Roll back to the previous working device ID
        setCameraDevice(prevDeviceId);
        useVoiceStore.setState({
          micToggleError: mediaErrorMessage(err, 'Camera device switch'),
        });
      });
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cameraDeviceId]);

  return {
    micEnabled,
    camEnabled,
    screenEnabled,
    canScreenShare,
    canCameraMic,
    cameraMicBlocked,
    cameraMicUnknown,
    isMacScreenShareUnknown,
    deafened,
    mediaError,
    setMediaError,
    staleCameraPrompt,
    deviceNotice,
    toggleMic,
    toggleCamera,
    toggleScreen,
    handleCameraFallback,
    screenShareTitle,
    cameraMicTitle,
  };
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
export function useTauriMediaRestore(
  localParticipant: WebRtcSession,
  onScreenShareInterrupted?: () => void,
): void {
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

        const lp = localParticipant;

        // Helper: find a publication by source from the participant's track map.
        const findPub = (source: TrackSource) => {
          for (const pub of lp.trackPublications.values()) {
            if (pub.source === source) return pub;
          }
          return undefined;
        };

        // Re-enable mic if it was on but the track ended
        if (lp.isMicrophoneEnabled) {
          const pub = findPub(TrackSource.Microphone);
          if (pub?.track?.mediaStreamTrack?.readyState === 'ended') {
            try {
              await lp.setMicrophoneEnabled(false);
              await lp.setMicrophoneEnabled(true);
            } catch { /* best effort */ }
          }
        }

        // Re-enable camera if it was on but the track ended
        if (lp.isCameraEnabled) {
          const pub = findPub(TrackSource.Camera);
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
          const pub = findPub(TrackSource.ScreenShare);
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
