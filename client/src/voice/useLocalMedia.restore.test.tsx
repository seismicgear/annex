import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TrackSource, type WebRtcSession } from '@/lib/webrtc';
import { useVoiceStore } from '@/stores/voice';
import { useTauriMediaRestore } from './useLocalMedia';

vi.mock('@/lib/tauri', () => ({ isTauri: () => true, invoke: vi.fn() }));

function endedSession() {
  return {
    // These getters are false for an OS-ended track. Restoration must look
    // at the retained publication, not require a live track to recover one.
    isMicrophoneEnabled: false, isCameraEnabled: false, isScreenShareEnabled: false,
    trackPublications: new Map(Object.values(TrackSource).map((source) => [source, {
      source, isMuted: false, track: { mediaStreamTrack: { readyState: 'ended' } },
    }])),
    setMicrophoneEnabled: vi.fn(async () => {}),
    setCameraEnabled: vi.fn(async () => {}),
    setScreenShareEnabled: vi.fn<(enabled: boolean) => Promise<void>>().mockResolvedValue(undefined),
  };
}

beforeEach(() => {
  vi.useFakeTimers();
  useVoiceStore.setState({ micMuted: false, inputDeviceId: 'selected-mic', cameraDeviceId: 'selected-camera' });
});
afterEach(() => vi.useRealTimers());

describe('desktop capture restoration', () => {
  it('recovers OS-ended microphone, camera and screen using the saved devices', async () => {
    const local = endedSession();
    renderHook(() => useTauriMediaRestore(local as unknown as WebRtcSession));
    await act(async () => {
      window.dispatchEvent(new Event('focus'));
      await vi.advanceTimersByTimeAsync(250);
    });
    expect(local.setMicrophoneEnabled).toHaveBeenCalledWith(true, { deviceId: 'selected-mic' });
    expect(local.setCameraEnabled).toHaveBeenCalledWith(true, { deviceId: 'selected-camera' });
    expect(local.setScreenShareEnabled).toHaveBeenCalledWith(true);
  });

  it('does not restore after the call component has unmounted', async () => {
    const local = endedSession();
    const { unmount } = renderHook(() => useTauriMediaRestore(local as unknown as WebRtcSession));
    window.dispatchEvent(new Event('focus'));
    unmount();
    await act(() => vi.advanceTimersByTimeAsync(250));
    expect(local.setMicrophoneEnabled).not.toHaveBeenCalled();
    expect(local.setCameraEnabled).not.toHaveBeenCalled();
    expect(local.setScreenShareEnabled).not.toHaveBeenCalled();
  });

  it('restores interrupted capture intent after ended publications have been removed', async () => {
    useVoiceStore.setState({ micMuted: true });
    const local = { ...endedSession(), trackPublications: new Map(),
      wasMediaInterrupted: (source: string) => source === TrackSource.Camera || source === TrackSource.Microphone };
    renderHook(() => useTauriMediaRestore(local as unknown as WebRtcSession));
    await act(async () => {
      window.dispatchEvent(new Event('focus'));
      await vi.advanceTimersByTimeAsync(250);
    });
    expect(local.setMicrophoneEnabled).toHaveBeenCalledWith(true, { deviceId: 'selected-mic' });
    expect(local.setCameraEnabled).toHaveBeenCalledWith(true, { deviceId: 'selected-camera' });
    expect(local.setScreenShareEnabled).not.toHaveBeenCalled();
  });

  it('keeps a deliberately muted microphone off and requests explicit screen resumption on failure', async () => {
    useVoiceStore.setState({ micMuted: true });
    const local = endedSession();
    local.setScreenShareEnabled.mockImplementation(async (enabled) => {
      if (enabled) throw new DOMException('User gesture required', 'NotAllowedError');
    });
    const interrupted = vi.fn();
    renderHook(() => useTauriMediaRestore(local as unknown as WebRtcSession, interrupted));
    await act(async () => {
      window.dispatchEvent(new Event('focus'));
      await vi.advanceTimersByTimeAsync(250);
    });
    expect(local.setMicrophoneEnabled).not.toHaveBeenCalled();
    expect(interrupted).toHaveBeenCalledTimes(1);
  });
});
