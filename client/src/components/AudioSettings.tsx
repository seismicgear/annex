/**
 * Audio & Video settings dialog.
 *
 * Lets users select input/output devices and adjust volume levels.
 * Settings are persisted to localStorage via the voice store.
 */

import { useState, useEffect } from 'react';
import { useVoiceStore } from '@/stores/voice';

interface DeviceInfo {
  deviceId: string;
  label: string;
  kind: MediaDeviceKind;
}

interface DeviceResult {
  devices: DeviceInfo[];
  permissionGranted: boolean;
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
  let permissionGranted = false;
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true, video: true }).catch(
      () => navigator.mediaDevices.getUserMedia({ audio: true }),
    );
    permissionGranted = true;
    stream.getTracks().forEach((t) => t.stop());
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
    setInputDevice,
    setOutputDevice,
    setInputVolume,
    setOutputVolume,
  } = useVoiceStore();

  const [devices, setDevices] = useState<DeviceInfo[]>([]);
  const [permissionGranted, setPermissionGranted] = useState(false);

  useEffect(() => {
    let cancelled = false;
    enumerateMediaDevices().then((result) => {
      if (cancelled) return;
      setPermissionGranted(result.permissionGranted);
      setDevices(result.devices);
    });
    return () => { cancelled = true; };
  }, []);

  const audioInputs = devices.filter((d) => d.kind === 'audioinput');
  const audioOutputs = devices.filter((d) => d.kind === 'audiooutput');
  const videoInputs = devices.filter((d) => d.kind === 'videoinput');

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog settings-dialog" onClick={(e) => e.stopPropagation()}>
        <h3>Audio & Video Settings</h3>

        {!permissionGranted && (
          <p className="settings-note">
            Grant microphone/camera access to see device names.
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
            Input Volume
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
        </div>

        <div className="settings-section">
          <label>
            Output Device (Speakers / Headphones)
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
            <select disabled>
              <option>
                {videoInputs.length > 0
                  ? videoInputs[0].label
                  : 'No camera detected'}
              </option>
              {videoInputs.map((d) => (
                <option key={d.deviceId} value={d.deviceId}>
                  {d.label}
                </option>
              ))}
            </select>
          </label>
          <p className="settings-note">
            Camera is toggled per-call from the media controls.
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
