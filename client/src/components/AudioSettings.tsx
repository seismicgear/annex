/**
 * Audio & Video settings dialog.
 *
 * Lets users select input/output devices and adjust volume levels.
 * Settings are persisted to localStorage via the voice store and applied
 * to the active LiveKit room in real time.
 *
 * Output device selection uses HTMLMediaElement.setSinkId() — a fallback
 * message is shown if the browser/webview does not support it.
 *
 * Input volume is labelled as OS-level only when we cannot apply gain
 * through the Web Audio API (which is the common case in LiveKit).
 */

import { useState, useEffect, useCallback } from 'react';
import { useVoiceStore } from '@/stores/voice';
import { isTauri } from '@/lib/tauri';

interface DeviceInfo {
  deviceId: string;
  label: string;
  kind: MediaDeviceKind;
  /** True when the label is a placeholder (permission not yet granted). */
  labelMissing: boolean;
}

interface DeviceResult {
  devices: DeviceInfo[];
  /** Per-kind permission status: 'granted' | 'prompt' | 'denied'. */
  micPermission: PermissionState | 'unknown';
  cameraPermission: PermissionState | 'unknown';
}

/** Check whether setSinkId is supported in this browser/webview. */
function isSinkIdSupported(): boolean {
  return typeof HTMLMediaElement !== 'undefined' &&
    typeof (HTMLMediaElement.prototype as Record<string, unknown>).setSinkId === 'function';
}

/** Query the Permissions API for a specific device kind, if supported. */
async function queryDevicePermission(kind: 'microphone' | 'camera'): Promise<PermissionState | 'unknown'> {
  try {
    if (navigator.permissions && typeof navigator.permissions.query === 'function') {
      const result = await navigator.permissions.query({ name: kind as PermissionName });
      return result.state;
    }
  } catch {
    // Permissions API may not support this name in all browsers
  }
  return 'unknown';
}

/**
 * Enumerate media devices without triggering getUserMedia on dialog open.
 *
 * Calls navigator.mediaDevices.enumerateDevices() first. Missing labels
 * are treated as a permissions limitation, not as missing hardware.
 */
async function enumerateMediaDevices(): Promise<DeviceResult> {
  if (!navigator.mediaDevices || typeof navigator.mediaDevices.enumerateDevices !== 'function') {
    return { devices: [], micPermission: 'unknown', cameraPermission: 'unknown' };
  }

  const [micPerm, cameraPerm] = await Promise.all([
    queryDevicePermission('microphone'),
    queryDevicePermission('camera'),
  ]);

  const list = await navigator.mediaDevices.enumerateDevices();
  return {
    micPermission: micPerm,
    cameraPermission: cameraPerm,
    devices: list
      .filter((d) => d.kind === 'audioinput' || d.kind === 'audiooutput' || d.kind === 'videoinput')
      .map((d) => ({
        deviceId: d.deviceId,
        label: d.label || `${d.kind} (${d.deviceId.slice(0, 8)})`,
        kind: d.kind,
        labelMissing: !d.label,
      })),
  };
}

/**
 * Request permission for a specific media type via getUserMedia, then
 * re-enumerate to get proper labels.
 */
async function requestDeviceAccess(audio: boolean, video: boolean): Promise<DeviceResult> {
  if (!navigator.mediaDevices?.getUserMedia) {
    throw new Error('getUserMedia not available');
  }
  const stream = await navigator.mediaDevices.getUserMedia({ audio, video });
  stream.getTracks().forEach((t) => t.stop());
  return enumerateMediaDevices();
}

export function AudioSettings({ onClose }: { onClose: () => void }) {
  const {
    inputDeviceId,
    outputDeviceId,
    inputVolume,
    outputVolume,
    cameraDeviceId,
    setInputDevice,
    setOutputDevice,
    setInputVolume,
    setOutputVolume,
    setCameraDevice,
  } = useVoiceStore();

  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [micPermission, setMicPermission] = useState<PermissionState | 'unknown'>('unknown');
  const [cameraPermission, setCameraPermission] = useState<PermissionState | 'unknown'>('unknown');
  const [enumError, setEnumError] = useState<string | null>(null);
  const [requestingMic, setRequestingMic] = useState(false);
  const [requestingCamera, setRequestingCamera] = useState(false);

  const applyResult = useCallback((result: DeviceResult) => {
    setDevices(result.devices);
    setMicPermission(result.micPermission);
    setCameraPermission(result.cameraPermission);
  }, []);

  const refreshDevices = useCallback(() => {
    let cancelled = false;
    setEnumError(null);
    enumerateMediaDevices()
      .then((result) => {
        if (cancelled) return;
        applyResult(result);
      })
      .catch((err) => {
        if (cancelled) return;
        setDevices([]);
        setEnumError(err instanceof Error ? err.message : 'Failed to enumerate media devices');
      });
    return () => { cancelled = true; };
  }, [applyResult]);

  // Enumerate on mount.
  useEffect(() => refreshDevices(), [refreshDevices]);

  // Re-enumerate when devices are plugged/unplugged.
  useEffect(() => {
    if (!navigator.mediaDevices?.addEventListener) return;

    const handler = () => { refreshDevices(); };
    navigator.mediaDevices.addEventListener('devicechange', handler);
    return () => {
      navigator.mediaDevices.removeEventListener('devicechange', handler);
    };
  }, [refreshDevices]);

  const handleRequestMicAccess = async () => {
    setRequestingMic(true);
    setEnumError(null);
    try {
      const result = await requestDeviceAccess(true, false);
      applyResult(result);
    } catch (err) {
      if (err instanceof DOMException && err.name === 'NotAllowedError') {
        setMicPermission('denied');
      } else {
        setEnumError(err instanceof Error ? err.message : 'Failed to request mic access');
      }
    } finally {
      setRequestingMic(false);
    }
  };

  const handleRequestCameraAccess = async () => {
    setRequestingCamera(true);
    setEnumError(null);
    try {
      const result = await requestDeviceAccess(false, true);
      applyResult(result);
    } catch (err) {
      if (err instanceof DOMException && err.name === 'NotAllowedError') {
        setCameraPermission('denied');
      } else {
        setEnumError(err instanceof Error ? err.message : 'Failed to request camera access');
      }
    } finally {
      setRequestingCamera(false);
    }
  };

  const audioInputs = devices.filter((d) => d.kind === 'audioinput');
  const audioOutputs = devices.filter((d) => d.kind === 'audiooutput');
  const videoInputs = devices.filter((d) => d.kind === 'videoinput');

  // Determine whether labels are missing per-kind
  const micLabelsMissing = audioInputs.length > 0 && audioInputs.every((d) => d.labelMissing);
  const cameraLabelsMissing = videoInputs.length > 0 && videoInputs.every((d) => d.labelMissing);

  // Determine per-kind permission states for UI
  const micNeedsPermission = micPermission === 'prompt' || (micPermission === 'unknown' && micLabelsMissing);
  const micDenied = micPermission === 'denied';
  const cameraNeedsPermission = cameraPermission === 'prompt' || (cameraPermission === 'unknown' && cameraLabelsMissing);
  const cameraDenied = cameraPermission === 'denied';

  const sinkIdSupported = isSinkIdSupported();

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog settings-dialog" onClick={(e) => e.stopPropagation()}>
        <h3>Audio & Video Settings</h3>

        {enumError && (
          <p className="settings-note settings-unsupported">
            Could not enumerate media devices: {enumError}.
            {isTauri() && <> This platform or webview may not expose media device APIs. Check your OS privacy settings.</>}
          </p>
        )}

        <div className="settings-section">
          <label>
            Input Device (Microphone)
            {micDenied && (
              <p className="settings-note settings-unsupported">
                Microphone permission was denied. Allow microphone access in your browser or OS settings to see device names.
              </p>
            )}
            {micNeedsPermission && !micDenied && (
              <p className="settings-note">
                Microphone permission has not been granted. Device names are hidden.
                <button
                  type="button"
                  className="inline-action-btn"
                  onClick={handleRequestMicAccess}
                  disabled={requestingMic}
                >
                  {requestingMic ? 'Requesting...' : 'Request microphone access'}
                </button>
              </p>
            )}
            <select
              value={inputDeviceId ?? ''}
              onChange={(e) => setInputDevice(e.target.value || null)}
            >
              <option value="">System Default</option>
              {audioInputs.map((d) => (
                <option key={d.deviceId} value={d.deviceId}>
                  {d.label}
                </option>
              ))}
            </select>
          </label>

          <label>
            Input Volume (OS-level)
            <div className="volume-row">
              <input
                type="range"
                min="0"
                max="100"
                value={inputVolume}
                onChange={(e) => setInputVolume(Number(e.target.value))}
                className="volume-slider"
              />
              <span className="volume-value">{inputVolume}%</span>
            </div>
          </label>
          <p className="settings-note">
            Microphone gain is controlled by your operating system. Adjust it in your OS sound settings.
          </p>
        </div>

        <div className="settings-section">
          <label>
            Output Device (Speakers / Headphones)
            {sinkIdSupported ? (
              <select
                value={outputDeviceId ?? ''}
                onChange={(e) => setOutputDevice(e.target.value || null)}
              >
                <option value="">System Default</option>
                {audioOutputs.map((d) => (
                  <option key={d.deviceId} value={d.deviceId}>
                    {d.label}
                  </option>
                ))}
              </select>
            ) : (
              <>
                <select disabled>
                  <option>System Default</option>
                </select>
                <p className="settings-note settings-unsupported">
                  Output device selection is not supported in this browser or webview. Audio will play through the system default output.
                </p>
              </>
            )}
          </label>

          <label>
            Output Volume
            <div className="volume-row">
              <input
                type="range"
                min="0"
                max="100"
                value={outputVolume}
                onChange={(e) => setOutputVolume(Number(e.target.value))}
                className="volume-slider"
              />
              <span className="volume-value">{outputVolume}%</span>
            </div>
          </label>
        </div>

        <div className="settings-section">
          <label>
            Camera
            {cameraDenied && (
              <p className="settings-note settings-unsupported">
                Camera permission was denied. Allow camera access in your browser or OS settings to see device names.
              </p>
            )}
            {cameraNeedsPermission && !cameraDenied && (
              <p className="settings-note">
                Camera permission has not been granted. Device names are hidden.
                <button
                  type="button"
                  className="inline-action-btn"
                  onClick={handleRequestCameraAccess}
                  disabled={requestingCamera}
                >
                  {requestingCamera ? 'Requesting...' : 'Request camera access'}
                </button>
              </p>
            )}
            {videoInputs.length > 0 ? (
              <select
                value={cameraDeviceId ?? ''}
                onChange={(e) => setCameraDevice(e.target.value || null)}
              >
                <option value="">System Default</option>
                {videoInputs.map((d) => (
                  <option key={d.deviceId} value={d.deviceId}>
                    {d.label}
                  </option>
                ))}
              </select>
            ) : cameraNeedsPermission || cameraDenied ? (
              <select disabled>
                <option>Grant camera permission to see devices</option>
              </select>
            ) : (
              <select disabled>
                <option>No camera detected</option>
              </select>
            )}
          </label>
          <p className="settings-note">
            Camera is activated per-call from the media controls. Select your preferred device here.
          </p>
        </div>

        <div className="dialog-actions">
          <button type="button" onClick={onClose} className="primary-btn">
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
