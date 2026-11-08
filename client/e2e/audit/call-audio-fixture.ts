import { expect, type Page } from '@playwright/test';

interface QuietCapture {
  source: MediaStream;
  context?: AudioContext;
  input?: MediaStreamAudioSourceNode;
  gain?: GainNode;
  output?: MediaStream;
  stopOutput?: () => void;
  closed: boolean;
  disposal?: Promise<void>;
}

interface QuietMicrophoneFixture {
  cleanup: () => Promise<void>;
  snapshot: () => {
    active: boolean;
    pending: number;
    nativeRestored: boolean;
    captures: {
      closed: boolean;
      context: AudioContextState | undefined;
      source: { id: string; state: MediaStreamTrackState; enabled: boolean; muted: boolean }[];
      output: { id: string; state: MediaStreamTrackState; enabled: boolean; muted: boolean }[];
    }[];
  };
}

type FixtureWindow = Window & { __annexAuditQuietMicrophone?: QuietMicrophoneFixture };

/** Select a quiet room for this capture without muting or faking call state. */
export function initializeQuietMicrophone(): void {
  const owner = window as FixtureWindow;
  const devices = navigator.mediaDevices;
  const nativeCapture = devices.getUserMedia;
  const captures = new Set<QuietCapture>();
  const failures: unknown[] = [];
  let active = true;
  let pending = 0;

  const dispose = (capture: QuietCapture): Promise<void> => {
    if (capture.closed) return capture.disposal ?? Promise.resolve();
    capture.closed = true;
    capture.stopOutput?.();
    capture.source.getTracks().forEach(track => track.stop());
    capture.input?.disconnect();
    capture.gain?.disconnect();
    capture.disposal = Promise.resolve().then(async () => {
      if (capture.context && capture.context.state !== 'closed') await capture.context.close();
    });
    return capture.disposal;
  };

  const quietCapture = async (constraints: MediaStreamConstraints): Promise<MediaStream> => {
    pending++;
    try {
      // Preserve native constraints, permission/device errors and acquisition.
      const source = await nativeCapture.call(devices, constraints);
      const capture: QuietCapture = { source, closed: false };
      captures.add(capture);
      try {
        if (!active) throw new DOMException('Quiet input fixture was closed', 'AbortError');
        const inputs = source.getAudioTracks();
        if (inputs.length !== 1 || inputs[0].readyState !== 'live' || !inputs[0].enabled) {
          throw new DOMException('Native microphone did not provide a live enabled track', 'NotReadableError');
        }
        // Inventory precedes resume: cleanup can race acquisition or startup.
        const context = capture.context = new AudioContext();
        const input = capture.input = context.createMediaStreamSource(source);
        const gain = capture.gain = context.createGain();
        gain.gain.value = 0;
        const destination = context.createMediaStreamDestination();
        const output = capture.output = destination.stream;
        const track = output.getAudioTracks()[0];
        const nativeStop = track.stop;
        capture.stopOutput = () => nativeStop.call(track);
        // stop() does not emit ended. Normal mute/replacement/disconnect must
        // also release the hidden native input and processing context.
        track.stop = () => {
          capture.stopOutput!();
          void dispose(capture).catch(error => failures.push(error));
        };
        inputs[0].addEventListener('ended', () => {
          if (capture.closed) return;
          void dispose(capture).then(() => {
            // Propagate actual source loss to the product's output listener.
            track.dispatchEvent(new Event('ended'));
          }).catch(error => failures.push(error));
        }, { once: true });
        input.connect(gain).connect(destination);
        await context.resume();
        if (!active || capture.closed || inputs[0].readyState !== 'live') {
          throw new DOMException('Native microphone ended during fixture startup', 'AbortError');
        }
        return output;
      } catch (error) {
        await dispose(capture);
        throw error;
      }
    } finally {
      pending--;
    }
  };

  const wrappedCapture: MediaDevices['getUserMedia'] = constraints => {
    // WebRtcSession acquires its microphone separately. Camera and mixed
    // requests are outside this fixture and retain their exact native stream.
    if (!constraints?.audio || constraints.video) return nativeCapture.call(devices, constraints);
    return quietCapture(constraints);
  };
  devices.getUserMedia = wrappedCapture;
  owner.__annexAuditQuietMicrophone = {
    cleanup: async () => {
      active = false;
      if (devices.getUserMedia === wrappedCapture) devices.getUserMedia = nativeCapture;
      await Promise.all([...captures].map(dispose));
      if (failures.length) throw new AggregateError(failures, 'Quiet input cleanup failed');
    },
    snapshot: () => {
      const tracks = (stream?: MediaStream) => (stream?.getTracks() ?? []).map(track => ({
        id: track.id, state: track.readyState, enabled: track.enabled, muted: track.muted,
      }));
      return {
        active, pending, nativeRestored: devices.getUserMedia === nativeCapture,
        captures: [...captures].map(capture => ({
          closed: capture.closed, context: capture.context?.state,
          source: tracks(capture.source), output: tracks(capture.output),
        })),
      };
    },
  };
}

export async function installQuietMicrophone(page: Page): Promise<void> {
  await page.addInitScript(initializeQuietMicrophone);
}

export async function quietMicrophoneState(page: Page) {
  return page.evaluate(() => (window as FixtureWindow).__annexAuditQuietMicrophone?.snapshot());
}

export async function clearQuietMicrophone(page: Page): Promise<void> {
  await page.evaluate(async () => {
    await (window as FixtureWindow).__annexAuditQuietMicrophone?.cleanup();
  });
}

/** Recheck the real connected, enabled microphone after layout stabilization. */
export async function expectQuietConnectedCall(page: Page, channel: string): Promise<void> {
  await expect(page.locator('.voice-connected-header')).toHaveText(`Voice Connected — ${channel}`);
  await expect(page.locator('.media-controls').getByTitle('Mute microphone', { exact: true })).toBeEnabled();
  await expect(page.locator('.local-media-status')).toContainText('Mic ON');
  const local = page.locator('.call-grid > .call-tile');
  await expect(local).toHaveCount(1);
  await expect(local.locator('.tile-name')).toHaveText('You');
  await expect.poll(async () => {
    const state = await quietMicrophoneState(page);
    const current = state?.captures.filter(capture => !capture.closed) ?? [];
    return state?.active && state.pending === 0 && current.length === 1 &&
      current.every(capture => capture.context === 'running' &&
        capture.source.length === 1 && capture.output.length === 1 &&
        [...capture.source, ...capture.output].every(track => track.state === 'live' && track.enabled && !track.muted)) &&
      state.captures.filter(capture => capture.closed).every(capture =>
        (!capture.context || capture.context === 'closed') &&
        [...capture.source, ...capture.output].every(track => track.state === 'ended'));
  }, { message: 'Quiet capture must retain live native input and a running, enabled output' }).toBe(true);
  await expect(local, 'Quiet microphone input must not show a speaking tile').not.toHaveClass(/\bspeaking\b/);
}
