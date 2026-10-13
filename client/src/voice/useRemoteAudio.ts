/** Apply call output preferences only to media rendered by the voice room. */
import { useEffect, useRef } from 'react';
import { useVoiceStore } from '@/stores/voice';

const CALL_AUDIO = 'audio[data-webrtc-remote], [data-testid="webrtc-room"] audio';

async function setSink(el: HTMLAudioElement, deviceId: string | null): Promise<void> {
  if (typeof el.setSinkId === 'function') {
    await el.setSinkId(deviceId ?? '');
  } else if (deviceId) {
    throw new Error('Output device selection is unavailable in this browser.');
  }
}

export function useRemoteAudio(): void {
  const deafened = useVoiceStore((s) => s.deafened);
  const outputVolume = useVoiceStore((s) => s.outputVolume);
  const outputDeviceId = useVoiceStore((s) => s.outputDeviceId);
  const applyRef = useRef<(() => void) | null>(null);

  useEffect(() => {
    let disposed = false;
    let acceptedDevice: string | null = null;
    let queue = Promise.resolve();
    const applied = new WeakMap<HTMLAudioElement, string | null>();
    const elements = () => [...document.querySelectorAll<HTMLAudioElement>(CALL_AUDIO)];

    const apply = () => {
      const { deafened, outputVolume } = useVoiceStore.getState();
      for (const el of elements()) {
        el.muted = deafened;
        el.volume = deafened ? 0 : Math.max(0, Math.min(1, outputVolume / 100));
      }
      // Serialize hardware operations: a slow earlier setSinkId must never
      // finish after the user's newest selection and silently override it.
      queue = queue.then(async () => {
        if (disposed) return;
        const device = useVoiceStore.getState().outputDeviceId;
        const audio = elements();
        if (!audio.length) return; // Validate once call audio actually exists.
        const route = async (target: string | null) => Promise.allSettled(audio.map(async (el) => {
          if (applied.has(el) && applied.get(el) === target) return;
          await setSink(el, target);
          applied.set(el, target);
        }));
        const results = await route(device);
        if (disposed || useVoiceStore.getState().outputDeviceId !== device) return;
        const failure = results.find((r) => r.status === 'rejected');
        if (!failure) {
          acceptedDevice = device;
          return;
        }

        // Restore every call element, including ones that accepted the failed
        // selection. A newly added participant can expose a lost saved device.
        let fallback = acceptedDevice === device ? null : acceptedDevice;
        let restored = await route(fallback);
        if (disposed || useVoiceStore.getState().outputDeviceId !== device) return;
        if (fallback && restored.some((r) => r.status === 'rejected')) {
          fallback = null;
          restored = await route(null);
          if (disposed || useVoiceStore.getState().outputDeviceId !== device) return;
        }
        acceptedDevice = fallback;
        const state = useVoiceStore.getState();
        state.setOutputDevice(fallback);
        const detail = failure.reason instanceof Error ? failure.reason.message : 'The device could not be selected.';
        const recovery = restored.some((r) => r.status === 'rejected')
          ? 'Call audio could not be restored. Check your output device and reconnect.'
          : fallback ? 'Restored the previous output device.' : 'Using the system default output.';
        state.setOutputDeviceError(`Output device change failed: ${detail} ${recovery}`);
      });
    };
    applyRef.current = apply;
    const observer = new MutationObserver((mutations) => {
      if (mutations.some((mutation) => [...mutation.addedNodes].some((node) =>
        node instanceof Element && (node.matches(CALL_AUDIO) || node.querySelector(CALL_AUDIO))
      ))) apply();
    });
    observer.observe(document.body, { childList: true, subtree: true });
    return () => {
      disposed = true;
      applyRef.current = null;
      observer.disconnect();
    };
  }, []);

  useEffect(() => {
    applyRef.current?.();
  }, [deafened, outputVolume, outputDeviceId]);
}
