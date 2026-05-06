/**
 * Apply the voice store's output preferences (output device, volume,
 * deafen) to every remote `<audio>` element rendered by the room.
 *
 * Existing elements are updated whenever any of the three preferences
 * changes; a `MutationObserver` watches `document.body` so that any
 * `<audio>` element added later (directly or as a descendant of a
 * dynamically inserted container) inherits the same preferences.
 */

import { useEffect, useRef } from 'react';
import { useVoiceStore } from '@/stores/voice';

/** Try to set the audio output device on an HTMLMediaElement (setSinkId).
 *  When `deviceId` is null/empty, resets to the system default ('').
 */
function trySetSinkId(el: HTMLMediaElement, deviceId: string | null): void {
  if (typeof (el as unknown as Record<string, unknown>).setSinkId === 'function') {
    ((el as unknown as Record<string, (...args: unknown[]) => Promise<void>>).setSinkId)(deviceId || '').catch(() => {});
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

export function useRemoteAudio(): void {
  const deafened = useVoiceStore((s) => s.deafened);
  const outputVolume = useVoiceStore((s) => s.outputVolume);
  const outputDeviceId = useVoiceStore((s) => s.outputDeviceId);

  // Apply output device and volume to all <audio> elements rendered by RemoteAudioRenderer.
  // Also handles deafen by muting those elements.
  // When outputDeviceId is null/empty, reset to system default via setSinkId('').
  useEffect(() => {
    const audioElements = document.querySelectorAll<HTMLAudioElement>('audio[data-webrtc-remote]');
    audioElements.forEach((el) => {
      applyAudioPrefs(el, deafened, outputVolume, outputDeviceId);
    });

    // Also handle any audio elements inside webrtc containers
    const containerAudioElements = document.querySelectorAll<HTMLAudioElement>('[data-testid="webrtc-room"] audio');
    containerAudioElements.forEach((el) => {
      applyAudioPrefs(el, deafened, outputVolume, outputDeviceId);
    });
  }, [deafened, outputVolume, outputDeviceId]);

  // Set up a MutationObserver to catch dynamically added audio elements.
  // Handles both direct <audio> nodes and container nodes with <audio> descendants.
  // Capture current values in a ref so the observer callback always uses fresh state.
  const deafenedRef = useRef(deafened);
  const outputVolumeRef = useRef(outputVolume);
  const outputDeviceIdRef = useRef(outputDeviceId);
  useEffect(() => {
    deafenedRef.current = deafened;
    outputVolumeRef.current = outputVolume;
    outputDeviceIdRef.current = outputDeviceId;
  });

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
