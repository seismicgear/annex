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
}

interface DeviceResult {
  devices: DeviceInfo[];
  permissionGranted: boolean;
}

/** Check whether setSinkId is supported in this browser/webview. */
function isSinkIdSupported(): boolean {
  return typeof HTMLMediaElement !== 'undefined' &&
    typeof (HTMLMediaElement.prototype as any).setSinkId === 'function';
}

/**
 * Enumerate media devices. Pure async — no React state calls.
 *
 * AUDIT-TAURI: In Tauri webviews, getUserMedia may behave differently than
 * in a browser. On Windows WebView2 without a PermissionRequested handler,
 * getUserMedia can silently return null (NotAllowedError). The catch block
 * handles this gracefully (limited labels shown), but verify on hardware
 * that the dialog prompts or auto-grants permission correctly.
 */
async function enumerateMediaDevices(): Promise<DeviceResult> {
  // Guard: media device APIs may be absent in some webviews/contexts
  if (!navigator.mediaDevices || typeof navigator.mediaDevices.enumerateDevices !== 'function') {
    return { devices: [], permissionGranted: false };
  }

  let permissionGranted = false;
  try {
    if (typeof navigator.mediaDevices.getUserMedia === 'function') {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true, video: true }).catch(
        () => navigator.mediaDevices.getUserMedia({ audio: true }),
      );
      permissionGranted = true;
      stream.getTracks().forEach((t) => t.stop());
    }
  } catch {
    // Permission denied — continue with limited labels.
  }
  const list = await navigator.mediaDevices.enumerateDevices();
  return {
    permissionGranted,
    devices: list
      .filter((d) => d.kind === 'audioinput' || d.kind === 'audiooutput' || d.kind === 'videoinput')
      .map((d) => ({
        deviceId: d.deviceId,
        label: d.label || `${d.kind} (${d.deviceId.slice(0, 8)})`,
        kind: d.kind,
      })),
  };
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
  const [permissionGranted, setPermissionGranted] = useState(false);
  const [enumError, setEnumError] = useState<string | null>(null);

  const refreshDevices = useCallback(() => {
    let cancelled = false;
    setEnumError(null);
    enumerateMediaDevices()
      .then((result) => {
        if (cancelled) return;
        setPermissionGranted(result.permissionGranted);
        setDevices(result.devices);
      })
      .catch((err) => {
        if (cancelled) return;
        setDevices([]);
        setPermissionGranted(false);
        setEnumError(err instanceof Error ? err.message : 'Failed to enumerate media devices');
      });
    return () => { cancelled = true; };
  }, []);

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

  const audioInputs = devices.filter((d) => d.kind === 'audioinput');
  const audioOutputs = devices.filter((d) => d.kind === 'audiooutput');
  const videoInputs = devices.filter((d) => d.kind === 'videoinput');

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

        {!permissionGranted && !enumError && (
          <p className="settings-note">
            Grant microphone/camera access to see device names.
            {isTauri() && (
              <> On desktop, your OS may need to grant this app camera and microphone
              permissions separately. Check your system privacy settings if devices are
              not listed below.</>
            )}
          </p>
        )}

        <div className="settings-section">
          <label>
            Input Device (Microphone)
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
