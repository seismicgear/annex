import { expect, test, type Page } from '@playwright/test';
import { mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { build } from 'vite';
import {
  clearQuietMicrophone, expectQuietConnectedCall, initializeQuietMicrophone,
  installQuietMicrophone, quietMicrophoneState,
} from './audit/call-audio-fixture';

// This spec belongs to the mandatory functional project, not the snapshot
// project. Only its browser uses controlled media; other functional tests keep
// their existing options. No SFU/server acceptance is claimed by this loopback.
test.use({
  permissions: ['microphone', 'camera'],
  launchOptions: { args: [
    '--use-fake-ui-for-media-stream', '--use-fake-device-for-media-stream',
    '--autoplay-policy=no-user-gesture-required',
    // Chromium's own peer tests enable loopback candidates for local peers.
    '--allow-loopback-in-peer-connection',
  ] },
});

const require = createRequire(import.meta.url);
const client = fileURLToPath(new URL('..', import.meta.url));
let temporary: string;
let javascript: string;
let stylesheet: string;

test.beforeAll(async () => {
  temporary = mkdtempSync(join(tmpdir(), 'annex-call-audio-proof-'));
  const entry = join(temporary, 'probe.js');
  // Bundle the actual session and UI components. The probe supplies only a
  // local real peer, parent rerenders, and an independent audio-energy meter.
  writeFileSync(entry, `
    import React from ${JSON.stringify(require.resolve('react'))};
    import { createRoot } from ${JSON.stringify(require.resolve('react-dom/client'))};
    import { WebRtcSession, TrackSource } from ${JSON.stringify(join(client, 'src/lib/webrtc.ts'))};
    import { ParticipantGrid } from ${JSON.stringify(join(client, 'src/voice/RemoteParticipants.tsx'))};
    import { MediaControls } from ${JSON.stringify(join(client, 'src/voice/MediaControls.tsx'))};
    import { LocalMediaStatus } from ${JSON.stringify(join(client, 'src/voice/VoiceDiagnostics.tsx'))};
    import { useVoiceStore } from ${JSON.stringify(join(client, 'src/stores/voice.ts'))};
    import ${JSON.stringify(join(client, 'src/index.css'))};
    import ${JSON.stringify(join(client, 'src/App.css'))};
    const errors = [], pendingIce = [];
    const report = error => errors.push(String(error));
    window.addEventListener('error', event => report(event.error ?? event.message));
    window.addEventListener('unhandledrejection', event => report(event.reason));
    const root = createRoot(document.getElementById('root'), { onUncaughtError: report });
    const gains = [];
    const nativeGain = AudioContext.prototype.createGain;
    AudioContext.prototype.createGain = function () {
      const node = nativeGain.call(this); gains.push({ node, context: this }); return node;
    };
    let session, peer, receiver, meter, outputMeter, noise, inputMeter;
    const nativeWarn = console.warn;
    console.warn = (...args) => {
      if (String(args[0]).startsWith('[webrtc]')) report(args.map(String).join(' '));
      nativeWarn(...args);
    };
    const render = () => {
      // Mount in-call controls after initial native acquisition/negotiation;
      // mounting them during connect would start a second mic-sync request.
      if (!session || session.connectionState !== 'connected') return;
      const media = { isMicrophoneEnabled: session.isMicrophoneEnabled,
        isCameraEnabled: session.isCameraEnabled, isScreenShareEnabled: session.isScreenShareEnabled };
      root.render(React.createElement('div', { className: 'voice-panel' },
        React.createElement('div', { className: 'voice-connected-header' },
          session.connectionState === 'connected' ? 'Voice Connected — audit-voice' : session.connectionState),
        React.createElement(LocalMediaStatus, media),
        React.createElement(ParticipantGrid, { session }),
        React.createElement(MediaControls, { ...media, localParticipant: session,
          onLeave: () => session.disconnect(), mediaStatus: null, platformWarnings: [] })));
    };
    const average = analyser => {
      if (!analyser) return null;
      const bytes = new Uint8Array(analyser.frequencyBinCount);
      analyser.getByteFrequencyData(bytes);
      return bytes.reduce((sum, value) => sum + value, 0) / bytes.length;
    };
    window.__annexCallProof = {
      start: async () => {
        peer = new RTCPeerConnection({ iceServers: [] });
        session = new WebRtcSession([], 'audit-voice', 'audit-local', {
          sendOffer: async (_, sdp) => {
            try {
              await peer.setRemoteDescription({ type: 'offer', sdp });
              for (const candidate of pendingIce.splice(0)) await peer.addIceCandidate(candidate);
              const answer = await peer.createAnswer(); await peer.setLocalDescription(answer);
              await session.handleAnswer(answer.sdp);
            } catch (error) { report(error); }
          },
          sendAnswer: () => {},
          sendIceCandidate: (_, candidate, sdpMid, sdpMLineIndex) => {
            const value = { candidate, sdpMid, sdpMLineIndex };
            if (peer.remoteDescription) void peer.addIceCandidate(value).catch(report);
            else pendingIce.push(value);
          },
        });
        peer.onicecandidate = event => {
          if (event.candidate) void session.handleIceCandidate(event.candidate.toJSON()).catch(report);
        };
        peer.ontrack = event => { if (event.track.kind === 'audio') receiver = event.receiver; };
        // Supply the local-only roster normally received from the server.
        // Connection, microphone and speaking state still come from the session.
        useVoiceStore.setState({ connectedChannelId: 'audit-voice',
          participantsByChannel: { 'audit-voice': [session.identity] } });
        session.onConnectionStateChange = render;
        session.onLocalTrackChanged = render;
        session.onRemoteTracksChanged = render;
        await session.connect(); render();
        const publication = session.trackPublications.get(TrackSource.Microphone);
        if (!publication) throw new Error('Actual microphone acquisition failed: ' + errors.join('; '));
        const track = publication.track.mediaStreamTrack;
        meter = new AudioContext(); outputMeter = meter.createAnalyser(); outputMeter.fftSize = 256;
        meter.createMediaStreamSource(new MediaStream([track])).connect(outputMeter);
        await meter.resume();
      },
      addNoise: () => {
        // Real broadband audio enters BEFORE the fixture gain. This proves that
        // silence is input control, and the unchanged detector still speaks
        // when nonquiet input is deliberately supplied for the negative case.
        const { context, node } = gains[0];
        const buffer = context.createBuffer(1, context.sampleRate, context.sampleRate);
        const samples = buffer.getChannelData(0); let seed = 12345;
        for (let i = 0; i < samples.length; i++) {
          seed = (1664525 * seed + 1013904223) >>> 0;
          samples[i] = (seed / 4294967296 - 0.5) * 0.8;
        }
        noise = context.createBufferSource(); noise.buffer = buffer; noise.loop = true;
        inputMeter = context.createAnalyser(); inputMeter.fftSize = 256;
        noise.connect(inputMeter); noise.connect(node); noise.start();
      },
      audible: value => { gains[0].node.gain.value = value ? 1 : 0; },
      read: async () => {
        const publication = session.trackPublications.get(TrackSource.Microphone);
        let received = 0;
        if (receiver) for (const item of (await receiver.getStats()).values()) {
          if (item.type === 'inbound-rtp') received += item.packetsReceived ?? 0;
        }
        return { connected: session.connectionState === 'connected', micOn: session.isMicrophoneEnabled,
          transport: { session: session.connectionState, peer: peer.connectionState,
            ice: peer.iceConnectionState, gathering: peer.iceGatheringState,
            candidates: (peer.localDescription?.sdp.match(/a=candidate:/g) ?? []).length },
          speaking: session.isSpeaking, inputEnergy: average(inputMeter), outputEnergy: average(outputMeter),
          outputId: publication?.track?.mediaStreamTrack.id, received, errors: [...errors] };
      },
      render,
      close: async () => {
        noise?.stop(); session?.disconnect(); peer?.close();
        if (meter && meter.state !== 'closed') await meter.close();
      },
    };
  `);
  const result = await build({
    configFile: false, root: client, logLevel: 'silent',
    define: { 'process.env.NODE_ENV': JSON.stringify('production') },
    esbuild: { jsx: 'automatic' },
    resolve: { alias: { '@': join(client, 'src') } },
    build: { write: false, minify: false, lib: { entry, formats: ['iife'], name: 'AnnexCallProof' } },
  });
  const outputs = (Array.isArray(result) ? result : [result]).flatMap(value => {
    if (!('output' in value)) throw new Error('Call proof requires a completed bundle');
    return value.output;
  });
  javascript = outputs.filter(value => value.type === 'chunk').map(value => value.code).join('\n');
  stylesheet = outputs.flatMap(value => value.type === 'asset' && value.fileName.endsWith('.css')
    ? [typeof value.source === 'string' ? value.source : Buffer.from(value.source).toString('utf8')] : []).join('\n');
});

test.afterAll(() => { if (temporary) rmSync(temporary, { recursive: true, force: true }); });

interface CallReading {
  connected: boolean; micOn: boolean; speaking: boolean; inputEnergy: number;
  outputEnergy: number; outputId?: string; received: number; errors: string[];
}
type ProbeWindow = Window & { __annexCallProof?: {
  start: () => Promise<void>; addNoise: () => void; audible: (value: boolean) => void;
  read: () => Promise<CallReading>; render: () => void; close: () => Promise<void>;
} };

async function openProbe(page: Page) {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  await page.route('**/__annex_call_probe__*', route => {
    const url = route.request().url();
    if (url.endsWith('.js')) return route.fulfill({ contentType: 'application/javascript; charset=utf-8', body: javascript });
    if (url.endsWith('.css')) return route.fulfill({ contentType: 'text/css; charset=utf-8', body: stylesheet });
    return route.fulfill({ contentType: 'text/html; charset=utf-8', body:
      '<!doctype html><meta charset="utf-8"><link rel="stylesheet" href="/__annex_call_probe__.css"><div id="root"></div>' +
      '<script src="/__annex_call_probe__.js"></script>' });
  });
  await page.goto('/__annex_call_probe__');
  expect(errors, 'Actual call proof must load without browser errors').toEqual([]);
  await expect.poll(() => page.evaluate(() => Boolean((window as ProbeWindow).__annexCallProof))).toBe(true);
}

const readCall = (page: Page) => page.evaluate(() => (window as ProbeWindow).__annexCallProof!.read());
const tileAppearance = (page: Page) => page.locator('.call-tile').evaluate(element => {
  const style = getComputedStyle(element);
  return { width: style.borderTopWidth, style: style.borderTopStyle,
    color: style.borderTopColor, shadow: style.boxShadow };
});
const finishTileTransition = (page: Page) => page.locator('.call-tile').evaluate(async element => {
  await Promise.all(element.getAnimations().map(animation => animation.finished));
});

test.afterEach(async ({ page }) => {
  if (test.info().status !== test.info().expectedStatus) {
    const reading = await readCall(page).catch(error => ({ unavailable: String(error) }));
    await test.info().attach('call-proof-failure-state', { body: JSON.stringify(reading, null, 2), contentType: 'application/json' });
  }
  await page.evaluate(async () => { await (window as ProbeWindow).__annexCallProof?.close(); });
  await clearQuietMicrophone(page);
});

test('quiet capture keeps real microphone and RTP live; real speaking is visible and rejected', async ({ page }) => {
  await installQuietMicrophone(page);
  await openProbe(page);
  await page.evaluate(() => (window as ProbeWindow).__annexCallProof!.start());
  await expect.poll(async () => {
    const state = await readCall(page);
    return { connected: state.connected, errors: state.errors };
  }).toEqual({ connected: true, errors: [] });
  await page.evaluate(() => (window as ProbeWindow).__annexCallProof!.addNoise());
  await expect.poll(async () => (await readCall(page)).inputEnergy).toBeGreaterThan(20);
  await expect.poll(async () => (await readCall(page)).received).toBeGreaterThan(0);
  const samples: CallReading[] = [];
  for (let i = 0; i < 8; i++) {
    await page.waitForTimeout(125); // Span the real detector's 100 ms intervals.
    const sample = await readCall(page); samples.push(sample);
    expect(sample).toMatchObject({ connected: true, micOn: true, speaking: false, outputEnergy: 0, errors: [] });
  }
  const state = await quietMicrophoneState(page);
  expect(samples[0].outputId).toBe(state!.captures[0].output[0].id);
  await expectQuietConnectedCall(page, 'audit-voice');
  const quietAppearance = await tileAppearance(page);
  expect(parseFloat(quietAppearance.width)).toBeGreaterThan(0);
  expect(quietAppearance.style).toBe('solid');
  expect(quietAppearance.color).not.toBe('rgba(0, 0, 0, 0)');
  await page.screenshot({ path: test.info().outputPath('quiet.png'), animations: 'disabled' });

  await page.evaluate(() => (window as ProbeWindow).__annexCallProof!.audible(true));
  await expect.poll(async () => (await readCall(page)).speaking).toBe(true);
  expect((await readCall(page)).outputEnergy).toBeGreaterThan(20);
  await page.evaluate(() => (window as ProbeWindow).__annexCallProof!.render());
  await expect(page.locator('.call-tile')).toHaveClass(/\bspeaking\b/);
  await finishTileTransition(page);
  const speakingAppearance = await tileAppearance(page);
  expect(parseFloat(speakingAppearance.width)).toBeGreaterThan(0);
  expect(speakingAppearance.style).toBe('solid');
  expect(speakingAppearance.color).not.toBe(quietAppearance.color);
  expect(speakingAppearance.color).not.toBe('rgba(0, 0, 0, 0)');
  expect(speakingAppearance.shadow).not.toBe(quietAppearance.shadow);
  expect(speakingAppearance.shadow).not.toBe('none');
  await page.screenshot({ path: test.info().outputPath('speaking.png'), animations: 'disabled' });
  await expect(expectQuietConnectedCall(page, 'audit-voice')).rejects.toThrow('Quiet microphone input must not show a speaking tile');

  await page.evaluate(() => (window as ProbeWindow).__annexCallProof!.audible(false));
  await expect.poll(async () => (await readCall(page)).speaking).toBe(false);
  await page.evaluate(() => (window as ProbeWindow).__annexCallProof!.render());
  await expectQuietConnectedCall(page, 'audit-voice');
  await finishTileTransition(page);
  const restoredAppearance = await tileAppearance(page);
  expect(restoredAppearance).toEqual(quietAppearance);
  await test.info().attach('speaking-presentation', {
    body: JSON.stringify({ quietAppearance, speakingAppearance, restoredAppearance }, null, 2),
    contentType: 'application/json',
  });
  await test.info().attach('real-audio-samples', { body: JSON.stringify(samples, null, 2), contentType: 'application/json' });
  // Exercise the real control's stop path, not only surface-finally cleanup.
  await page.locator('.media-controls').getByTitle('Mute microphone', { exact: true }).click();
  await expect(page.locator('.local-media-status')).toContainText('Mic OFF');
  await expect.poll(async () => {
    const capture = (await quietMicrophoneState(page))!.captures[0];
    return capture.closed && capture.context === 'closed' &&
      [...capture.source, ...capture.output].every(track => track.state === 'ended');
  }).toBe(true);
  await clearQuietMicrophone(page);
  expect(await quietMicrophoneState(page)).toMatchObject({ active: false, pending: 0, nativeRestored: true });
});

test('native constraints and rejections survive; video and mixed streams pass through unchanged', async ({ page }) => {
  await openProbe(page);
  const result = await page.evaluate(async () => {
    const devices = navigator.mediaDevices;
    const original = devices.getUserMedia;
    const source = await original.call(devices, { audio: true, video: true });
    const failure = new DOMException('native capture denied', 'NotAllowedError');
    let requested: MediaStreamConstraints | undefined;
    const delegate: MediaDevices['getUserMedia'] = constraints => {
      requested = constraints;
      return constraints?.video ? Promise.resolve(source) : Promise.reject(failure);
    };
    devices.getUserMedia = delegate;
    (window as Window & { __audioCase?: unknown }).__audioCase = { original, source, failure, delegate, request: () => requested };
    return source.getTracks().length;
  });
  expect(result).toBe(2);
  await page.evaluate(initializeQuietMicrophone);
  const proof = await page.evaluate(async () => {
    const state = (window as unknown as Window & { __audioCase: {
      source: MediaStream; failure: DOMException; request: () => MediaStreamConstraints | undefined;
    } }).__audioCase;
    const video = { video: true }, mixed = { audio: true, video: true };
    const a = await navigator.mediaDevices.getUserMedia(video);
    const exactVideo = state.request() === video && a === state.source;
    const b = await navigator.mediaDevices.getUserMedia(mixed);
    const exactMixed = state.request() === mixed && b === state.source;
    const audio = { audio: { deviceId: { exact: 'native-device' }, echoCancellation: true } };
    let sameError = false;
    try { await navigator.mediaDevices.getUserMedia(audio); } catch (error) { sameError = error === state.failure; }
    return { exactVideo, exactMixed, exactAudioConstraints: state.request() === audio, sameError };
  });
  expect(proof).toEqual({ exactVideo: true, exactMixed: true, exactAudioConstraints: true, sameError: true });
  expect(await quietMicrophoneState(page)).toMatchObject({ pending: 0, captures: [] });
  await clearQuietMicrophone(page);
  expect(await page.evaluate(() => {
    const state = (window as unknown as Window & { __audioCase: { delegate: MediaDevices['getUserMedia']; source: MediaStream } }).__audioCase;
    const result = navigator.mediaDevices.getUserMedia === state.delegate && state.source.getTracks().every(track => track.readyState === 'live');
    state.source.getTracks().forEach(track => track.stop()); return result;
  })).toBe(true);
});

for (const invalid of ['missing', 'ended'] as const) {
  test(`a ${invalid} native audio input cannot become a healthy quiet track`, async ({ page }) => {
    await openProbe(page);
    await page.evaluate(async invalid => {
      const source = invalid === 'missing' ? new MediaStream() : await navigator.mediaDevices.getUserMedia({ audio: true });
      source.getTracks().forEach(track => track.stop());
      navigator.mediaDevices.getUserMedia = async () => source;
    }, invalid);
    await page.evaluate(initializeQuietMicrophone);
    expect(await page.evaluate(async () => {
      try { await navigator.mediaDevices.getUserMedia({ audio: true }); return 'unexpected success'; }
      catch (error) { return (error as DOMException).name; }
    })).toBe('NotReadableError');
    expect(await quietMicrophoneState(page)).toMatchObject({ pending: 0, captures: [{ closed: true, output: [] }] });
  });
}

test('cleanup rejects and releases native acquisition that resolves later', async ({ page }) => {
  await openProbe(page);
  await page.evaluate(async () => {
    const source = await navigator.mediaDevices.getUserMedia({ audio: true });
    let resolve!: (stream: MediaStream) => void;
    const delayed = new Promise<MediaStream>(done => { resolve = done; });
    navigator.mediaDevices.getUserMedia = () => delayed;
    (window as Window & { __lateAudio?: unknown }).__lateAudio = { source, resolve };
  });
  await page.evaluate(initializeQuietMicrophone);
  await page.evaluate(() => {
    const state = (window as unknown as Window & { __lateAudio: { result?: Promise<string> } }).__lateAudio;
    state.result = navigator.mediaDevices.getUserMedia({ audio: true }).then(() => 'unexpected success', error => error.name);
  });
  await clearQuietMicrophone(page);
  expect(await page.evaluate(async () => {
    const state = (window as unknown as Window & { __lateAudio: { source: MediaStream; resolve: (stream: MediaStream) => void; result: Promise<string> } }).__lateAudio;
    state.resolve(state.source);
    return { result: await state.result, ended: state.source.getTracks().every(track => track.readyState === 'ended') };
  })).toEqual({ result: 'AbortError', ended: true });
  expect(await quietMicrophoneState(page)).toMatchObject({ active: false, pending: 0, nativeRestored: true, captures: [{ closed: true, output: [] }] });
});

test('processing startup failure releases native and output tracks', async ({ page }) => {
  await openProbe(page);
  await page.evaluate(() => { AudioContext.prototype.resume = async () => { throw new Error('proof resume failure'); }; });
  await page.evaluate(initializeQuietMicrophone);
  expect(await page.evaluate(async () => {
    try { await navigator.mediaDevices.getUserMedia({ audio: true }); return 'unexpected success'; }
    catch (error) { return (error as Error).message; }
  })).toBe('proof resume failure');
  const capture = (await quietMicrophoneState(page))!.captures[0];
  expect(capture).toMatchObject({ closed: true, context: 'closed' });
  expect([...capture.source, ...capture.output].every(track => track.state === 'ended')).toBe(true);
});

test('unexpected native source loss ends the output and reaches its consumer', async ({ page }) => {
  await openProbe(page);
  await page.evaluate(() => {
    const native = navigator.mediaDevices.getUserMedia;
    navigator.mediaDevices.getUserMedia = async constraints => {
      const source = await native.call(navigator.mediaDevices, constraints);
      (window as Window & { __sourceAudio?: MediaStream }).__sourceAudio = source;
      return source;
    };
  });
  await page.evaluate(initializeQuietMicrophone);
  await page.evaluate(async () => {
    const output = await navigator.mediaDevices.getUserMedia({ audio: true });
    const probe = window as Window & { __outputAudio?: MediaStream; __endedEvents?: number };
    probe.__outputAudio = output; probe.__endedEvents = 0;
    output.getAudioTracks()[0].addEventListener('ended', () => { probe.__endedEvents!++; });
    // Native stop does not emit ended. Explicitly deliver the source-loss event
    // as fault injection; this is not a physical-device-removal acceptance test.
    const source = (window as unknown as Window & { __sourceAudio: MediaStream }).__sourceAudio.getAudioTracks()[0];
    source.stop(); source.dispatchEvent(new Event('ended'));
  });
  await expect.poll(() => page.evaluate(() => (window as unknown as Window & { __endedEvents: number }).__endedEvents)).toBe(1);
  const capture = (await quietMicrophoneState(page))!.captures[0];
  expect(capture).toMatchObject({ closed: true, context: 'closed' });
  expect([...capture.source, ...capture.output].every(track => track.state === 'ended')).toBe(true);
});
