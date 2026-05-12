/**
 * Status banners and indicators surrounding the voice room:
 *
 *   • PlatformMediaWarning — Linux PipeWire / portal guidance shown
 *     above the join button when the platform reports media warnings.
 *   • LocalMediaStatus    — mic / camera / screen-share status pills
 *     shown above the in-call controls.
 */

import type { PlatformMediaStatus } from '@/lib/tauri';

/** Platform media warning banner (Linux PipeWire / portal issues). */
export function PlatformMediaWarning({ mediaStatus }: { mediaStatus: PlatformMediaStatus | null }) {
  if (!mediaStatus || mediaStatus.warnings.length === 0) return null;
  return (
    <div className="voice-error" role="status">
      {mediaStatus.warnings.map((w, i) => (
        <p key={i} className="voice-setup-hint">{w}</p>
      ))}
    </div>
  );
}

interface LocalMediaStatusProps {
  isMicrophoneEnabled: boolean;
  isCameraEnabled: boolean;
  isScreenShareEnabled: boolean;
}

/** Local media status bar shown above the controls. */
export function LocalMediaStatus({
  isMicrophoneEnabled,
  isCameraEnabled,
  isScreenShareEnabled,
}: LocalMediaStatusProps) {
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
