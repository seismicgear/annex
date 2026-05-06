/**
 * In-call controls bar: mic/camera/screen-share toggle buttons plus the
 * inline error/notice banners that surround them. State and handlers
 * come from `useLocalMedia`; this component is purely presentational.
 */

import type { WebRtcSession } from '@/lib/webrtc';
import type { PlatformMediaStatus } from '@/lib/tauri';
import { useLocalMedia } from './useLocalMedia';

interface MediaControlsProps {
  localParticipant: WebRtcSession;
  isMicrophoneEnabled: boolean;
  isCameraEnabled: boolean;
  isScreenShareEnabled: boolean;
  onLeave: () => void;
  mediaStatus: PlatformMediaStatus | null;
  platformWarnings: string[];
}

export function MediaControls({
  localParticipant,
  isMicrophoneEnabled,
  isCameraEnabled,
  isScreenShareEnabled,
  onLeave,
  mediaStatus,
  platformWarnings,
}: MediaControlsProps) {
  const {
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
  } = useLocalMedia({
    localParticipant,
    isMicrophoneEnabled,
    isCameraEnabled,
    isScreenShareEnabled,
    mediaStatus,
    platformWarnings,
  });

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
      {staleCameraPrompt && (
        <div className="media-error stale-camera-recovery" role="alert">
          <span>Saved camera not found or disconnected.</span>
          <div className="stale-camera-actions">
            <button onClick={handleCameraFallback} className="stale-camera-btn">
              Use default camera
            </button>
          </div>
        </div>
      )}
      {cameraMicBlocked && (
        <div className="media-error" role="alert">
          Camera and microphone are blocked. Check your OS privacy settings to allow access.
        </div>
      )}
      {cameraMicUnknown && !cameraMicBlocked && (
        <div className="device-notice" role="status">
          Camera/microphone permission status unknown — you may need to grant access in your OS settings.
        </div>
      )}
      {isMacScreenShareUnknown && !screenEnabled && (
        <div className="device-notice" role="status">
          Screen Recording permission may be required on macOS. Enable it in System Settings if sharing fails.
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
