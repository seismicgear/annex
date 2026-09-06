/**
 * A mute that fails has to say so where the user pressed it.
 *
 * `micMuted` in the voice store is the intent; `useLocalMedia` reconciles it
 * onto the real WebRTC participant. That is what makes the status-bar button
 * and the in-call control bar work through one path — neither calls
 * `setMicrophoneEnabled` itself.
 *
 * When the reconcile fails the hook restores `micMuted` to the participant's
 * real state, so the button flips back. It reported the reason into
 * `mediaError`, which is `useState` inside this hook and renders only in
 * `MediaControls` — inside the call panel. A user who muted from the status
 * bar while on the Federation or Events tab saw the button flip back with no
 * explanation anywhere on screen. `micToggleError` is the store slot the
 * status-bar strip renders, right beside the control that failed, and nothing
 * on this path was setting it.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';

vi.mock('@/lib/tauri', () => ({
  isTauri: () => false,
  invoke: vi.fn(),
}));

function participant(enabled: boolean, fail: boolean) {
  return {
    isMicrophoneEnabled: enabled,
    isCameraEnabled: false,
    isScreenShareEnabled: false,
    setMicrophoneEnabled: vi.fn(async () => {
      if (fail) throw new Error('device lost');
    }),
    setCameraEnabled: vi.fn(async () => {}),
    setScreenShareEnabled: vi.fn(async () => {}),
    getTrackPublication: vi.fn(() => undefined),
    trackPublications: new Map(),
    on: vi.fn(),
    off: vi.fn(),
  };
}

async function renderMedia(lp: ReturnType<typeof participant>) {
  const { useLocalMedia } = await import('./useLocalMedia');
  return renderHook(() =>
    useLocalMedia({
      localParticipant: lp as never,
      isMicrophoneEnabled: lp.isMicrophoneEnabled,
      isCameraEnabled: false,
      isScreenShareEnabled: false,
      mediaStatus: null,
      platformWarnings: [],
    }),
  );
}

describe('useLocalMedia — reconciling micMuted onto the participant', () => {
  beforeEach(() => {
    vi.resetModules();
    vi.clearAllMocks();
  });

  it('applies a mute requested through the store', async () => {
    const { useVoiceStore } = await import('@/stores/voice');
    useVoiceStore.setState({ micMuted: true });
    const lp = participant(true, false);

    await renderMedia(lp);

    await waitFor(() => {
      expect(lp.setMicrophoneEnabled).toHaveBeenCalledWith(false);
    });
  });

  it('restores micMuted and reports into the store slot the status bar renders', async () => {
    const { useVoiceStore } = await import('@/stores/voice');
    useVoiceStore.setState({ micMuted: true, micToggleError: null });
    const lp = participant(true, true);

    await renderMedia(lp);

    await waitFor(() => {
      // The participant is still live, so the button must go back to unmuted.
      expect(useVoiceStore.getState().micMuted).toBe(false);
    });
    // The reason has to reach the store, not only this hook's local state:
    // the control that was pressed may be the status bar's, and that renders
    // `micToggleError`.
    expect(useVoiceStore.getState().micToggleError).toMatch(/device lost/i);
  });

  it('never mutes a participant whose state already matches the intent', async () => {
    const { useVoiceStore } = await import('@/stores/voice');
    useVoiceStore.setState({ micMuted: false });
    const lp = participant(true, false);

    await renderMedia(lp);

    // Asserting "not called at all" would be wrong: the input-device effect
    // re-applies the mic on mount for its own reasons. What must never happen
    // is this effect disabling a microphone the store says should be live.
    expect(lp.setMicrophoneEnabled).not.toHaveBeenCalledWith(false);
  });
});
