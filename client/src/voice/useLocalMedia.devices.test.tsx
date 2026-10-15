import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useVoiceStore } from '@/stores/voice';
import { useLocalMedia } from './useLocalMedia';
import type { WebRtcSession } from '@/lib/webrtc';

vi.mock('@/lib/tauri', () => ({ isTauri: () => false, invoke: vi.fn() }));

function participant() {
  return {
    isMicrophoneEnabled: true, isCameraEnabled: true,
    setMicrophoneEnabled: vi.fn(async () => {}),
    setCameraEnabled: vi.fn(async () => {}),
    setScreenShareEnabled: vi.fn(async () => {}),
  };
}

function renderMedia(localParticipant: ReturnType<typeof participant>) {
  return renderHook(() => useLocalMedia({
    localParticipant: localParticipant as unknown as WebRtcSession,
    isMicrophoneEnabled: localParticipant.isMicrophoneEnabled,
    isCameraEnabled: localParticipant.isCameraEnabled,
    isScreenShareEnabled: false, mediaStatus: null, platformWarnings: [],
  }));
}

describe('live media device selection', () => {
  beforeEach(() => {
    useVoiceStore.setState({
      inputDeviceId: 'original-mic', cameraDeviceId: 'original-camera',
      micMuted: false, micToggleError: null,
    });
  });

  it('does not interrupt microphone or camera when the call controls mount', () => {
    const local = participant();
    renderMedia(local);
    expect(local.setMicrophoneEnabled).not.toHaveBeenCalled();
    expect(local.setCameraEnabled).not.toHaveBeenCalled();
  });

  it('replaces microphone and camera without stopping the working capture first', async () => {
    const local = participant();
    renderMedia(local);
    act(() => {
      useVoiceStore.getState().setInputDevice('next-mic');
      useVoiceStore.getState().setCameraDevice('next-camera');
    });
    await waitFor(() => {
      expect(local.setMicrophoneEnabled).toHaveBeenCalledExactlyOnceWith(true, { deviceId: 'next-mic' });
      expect(local.setCameraEnabled).toHaveBeenCalledExactlyOnceWith(true, { deviceId: 'next-camera' });
    });
  });

  it('restores failed selections without republishing the still-working devices', async () => {
    const local = participant();
    local.setMicrophoneEnabled.mockRejectedValueOnce(new Error('microphone missing'));
    local.setCameraEnabled.mockRejectedValueOnce(new Error('camera missing'));
    renderMedia(local);
    act(() => {
      useVoiceStore.getState().setInputDevice('missing-mic');
      useVoiceStore.getState().setCameraDevice('missing-camera');
    });
    await waitFor(() => {
      expect(useVoiceStore.getState().inputDeviceId).toBe('original-mic');
      expect(useVoiceStore.getState().cameraDeviceId).toBe('original-camera');
    });
    expect(local.setMicrophoneEnabled).toHaveBeenCalledTimes(1);
    expect(local.setCameraEnabled).toHaveBeenCalledTimes(1);
    expect(useVoiceStore.getState().micMuted).toBe(false);
    expect(useVoiceStore.getState().micToggleError).toMatch(/missing/);
  });

  it('does not overwrite a newer selection when an earlier device request fails late', async () => {
    let reject!: (reason: Error) => void;
    const firstRequest = new Promise<void>((_, failed) => { reject = failed; });
    const local = participant();
    local.setCameraEnabled.mockReturnValueOnce(firstRequest);
    renderMedia(local);
    act(() => useVoiceStore.getState().setCameraDevice('slow-camera'));
    act(() => useVoiceStore.getState().setCameraDevice('newest-camera'));
    await act(async () => { reject(new Error('old request failed')); });
    expect(useVoiceStore.getState().cameraDeviceId).toBe('newest-camera');
    expect(useVoiceStore.getState().micToggleError).toBeNull();
  });

  it('unmutes the saved microphone when the status bar changes the mute preference', async () => {
    useVoiceStore.setState({ micMuted: true });
    const local = participant();
    local.isMicrophoneEnabled = false;
    renderMedia(local);
    act(() => useVoiceStore.getState().setMicMuted(false));
    await waitFor(() => {
      expect(local.setMicrophoneEnabled).toHaveBeenCalledExactlyOnceWith(true, { deviceId: 'original-mic' });
    });
  });
});
