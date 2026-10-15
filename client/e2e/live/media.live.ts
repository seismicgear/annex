import { test, expect, type Browser, type BrowserContext, type Page } from '@playwright/test';
import { completeStartup } from '../helpers';
import type { Channel } from '../../src/types';

/**
 * Native capture -> actual WebRTC -> Rust SFU -> independent browser decoder.
 * The only instrumentation observes native peer connections and getStats().
 * No routes, signaling, capture results, application stores, or speech engines
 * are replaced. Controlled Chromium input is explicitly distinguished from
 * opt-in physical camera/microphone acceptance in the evidence.
 */
type Source = 'audio' | 'video' | 'screen';
interface Inbound {
  key: string;
  trackId: string;
  kind: string;
  packets: number;
  bytes: number;
  frames: number;
  samples: number;
  energy: number;
  width: number;
  height: number;
  codec: string;
  receiverObjectId: number | null;
  mid: string | null;
  keyFrames: number;
  framesDropped: number;
  jitterBufferDelay: number;
  jitterBufferEmittedCount: number;
  estimatedPlayoutTimestamp: number | null;
}
interface NativeVideoFrames {
  callbacks: number;
  mediaTime: number;
  presentedFrames: number;
  width: number;
  height: number;
  rtpTimestamp: number | null;
}
interface Capture {
  id: string;
  kind: string;
  state: MediaStreamTrackState;
  enabled: boolean;
  displaySurface: string | null;
}
interface MediaSnapshot {
  at: string;
  peers: Array<{ id: number; connection: RTCPeerConnectionState; signaling: RTCSignalingState }>;
  inbound: Inbound[];
  captures: Capture[];
  videos: Array<{ trackId: string; width: number; height: number; frames: number; paused: boolean; time: number;
    objectId: number; readyState: MediaStreamTrackState; muted: boolean; enabled: boolean;
    elementReadyState: number; elementNetworkState: number; mediaError: number | null; streamActive: boolean;
    matchingReceivers: Array<{ peer: number; mid: string | null; objectId: number }>;
    nativeFrames: NativeVideoFrames | null }>;
  receivers: Array<{ peer: number; mid: string | null; direction: RTCRtpTransceiverDirection | null;
    objectId: number; trackId: string; kind: string; readyState: MediaStreamTrackState; muted: boolean; enabled: boolean }>;
  audio: Array<{ trackId: string; muted: boolean; volume: number; paused: boolean; time: number }>;
  captureEvents: Array<{ event: string; id: string; kind: string; at: number; stack?: string; width?: number; height?: number; channels?: number }>;
  receiverEvents: Array<{ event: string; peer: number; objectId: number; trackId: string; kind: string;
    at: number; mid: string | null; readyState: MediaStreamTrackState; muted: boolean; enabled: boolean; streamIds?: string[] }>;
}
interface Probe {
  peers: RTCPeerConnection[];
  captures: Map<string, MediaStreamTrack>;
  captureEvents: MediaSnapshot['captureEvents'];
  receiverEvents: MediaSnapshot['receiverEvents'];
  trackObjects: WeakMap<MediaStreamTrack, number>;
  nextTrackObjectId: number;
  videoFrames: WeakMap<HTMLVideoElement, NativeVideoFrames>;
}
declare global { interface Window { __annexMediaAcceptance?: Probe } }
interface Actor {
  context: BrowserContext;
  page: Page;
  headers: Record<string, string>;
  pseudonym: string;
  errors: string[];
  consoleErrors: string[];
  captions: Array<{ at: string; channelId: string; speakerPseudonym: string; text: string }>;
}
const baseURL = process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000';
const mode = process.env.ANNEX_MEDIA_SOURCE === 'physical' ? 'physical' : 'controlled';
const actors: Actor[] = [];
const observations: Array<{ step: string; observer: string; snapshot: MediaSnapshot }> = [];
let founder: Actor;
let peer: Actor;
let outsider: Actor;

async function actor(browser: Browser): Promise<Actor> {
  const context = await browser.newContext({ baseURL, permissions: ['microphone', 'camera'] });
  await context.addInitScript(() => {
    const probe: Probe = { peers: [], captures: new Map(), captureEvents: [], receiverEvents: [],
      trackObjects: new WeakMap(), nextTrackObjectId: 1, videoFrames: new WeakMap() };
    window.__annexMediaAcceptance = probe;
    const objectId = (track: MediaStreamTrack) => {
      let id = probe.trackObjects.get(track);
      if (id === undefined) { id = probe.nextTrackObjectId++; probe.trackObjects.set(track, id); }
      return id;
    };
    const observedReceivers = new WeakSet<MediaStreamTrack>();
    const NativePeer = window.RTCPeerConnection;
    window.RTCPeerConnection = class ObservedPeer extends NativePeer {
      constructor(configuration?: RTCConfiguration) {
        super(configuration);
        probe.peers.push(this);
        this.addEventListener('track', event => {
          const track = event.track;
          const log = (name: string, streamIds?: string[]) => probe.receiverEvents.push({
            event: name, peer: probe.peers.indexOf(this), objectId: objectId(track), trackId: track.id,
            kind: track.kind, at: Date.now(), mid: event.transceiver.mid,
            readyState: track.readyState, muted: track.muted, enabled: track.enabled, streamIds,
          });
          log('track', event.streams.map(stream => stream.id));
          if (!observedReceivers.has(track)) {
            observedReceivers.add(track);
            for (const name of ['mute', 'unmute', 'ended']) track.addEventListener(name, () => log(name));
          }
        });
      }
    };
    // Observe native presentation callbacks without changing playback or media.
    new MutationObserver(() => {
      for (const element of document.querySelectorAll<HTMLVideoElement>('.call-grid video')) {
        if (probe.videoFrames.has(element) || !element.requestVideoFrameCallback) continue;
        const frames: NativeVideoFrames = { callbacks: 0, mediaTime: 0, presentedFrames: 0,
          width: 0, height: 0, rtpTimestamp: null };
        probe.videoFrames.set(element, frames);
        const onFrame: VideoFrameRequestCallback = (_now, metadata) => {
          frames.callbacks++;
          Object.assign(frames, { mediaTime: metadata.mediaTime, presentedFrames: metadata.presentedFrames,
            width: metadata.width, height: metadata.height, rtpTimestamp: metadata.rtpTimestamp ?? null });
          if (element.isConnected) element.requestVideoFrameCallback(onFrame);
        };
        element.requestVideoFrameCallback(onFrame);
      }
    }).observe(document, { childList: true, subtree: true });
    const nativeStop = MediaStreamTrack.prototype.stop;
    MediaStreamTrack.prototype.stop = function () {
      probe.captureEvents.push({ event: 'stop-called', id: this.id, kind: this.kind, at: Date.now(),
        stack: new Error().stack?.split('\n').slice(1, 8).join('\n') });
      return nativeStop.call(this);
    };
    const nativeCapture = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
    navigator.mediaDevices.getUserMedia = async constraints => {
      const stream = await nativeCapture(constraints);
      for (const track of stream.getTracks()) {
        probe.captures.set(track.id, track);
        const settings = track.getSettings();
        probe.captureEvents.push({ event: 'capture-resolved', id: track.id, kind: track.kind, at: Date.now(),
          width: settings.width, height: settings.height, channels: settings.channelCount });
        track.addEventListener('ended', () => probe.captureEvents.push({ event: 'native-ended', id: track.id, kind: track.kind, at: Date.now() }));
      }
      return stream;
    };
  });
  const page = await context.newPage();
  const value: Actor = { context, page, headers: {}, pseudonym: '', errors: [], consoleErrors: [], captions: [] };
  page.on('pageerror', error => value.errors.push(error.message));
  page.on('console', message => {
    if (message.type() === 'error' || message.type() === 'warning') value.consoleErrors.push(message.text());
  });
  page.on('request', request => {
    if (new URL(request.url()).origin !== new URL(baseURL).origin) return;
    const headers = request.headers();
    if (headers.authorization && headers['x-annex-zk-proof']) {
      value.headers = Object.fromEntries(Object.entries(headers).filter(([key]) =>
        key === 'authorization' || key.startsWith('x-annex-')));
    }
  });
  page.on('websocket', socket => socket.on('framereceived', frame => {
    // Observe only delivered captions; never retain auth or other WS frames.
    try {
      const event = JSON.parse(frame.payload.toString()) as Record<string, unknown>;
      if (event.type === 'transcription' && typeof event.channelId === 'string'
        && typeof event.speakerPseudonym === 'string' && typeof event.text === 'string') {
        value.captions.push({ at: new Date().toISOString(), channelId: event.channelId,
          speakerPseudonym: event.speakerPseudonym, text: event.text });
      }
    } catch { /* A non-JSON websocket frame is unrelated to caption evidence. */ }
  }));
  actors.push(value);
  await completeStartup(page);
  value.pseudonym = await page.evaluate(async () => new Promise<string>((resolve, reject) => {
    const opening = indexedDB.open('annex-identity');
    opening.onerror = () => reject(new Error('Identity read failed'));
    opening.onsuccess = () => {
      const db = opening.result;
      const request = db.transaction('identities', 'readonly').objectStore('identities').getAll();
      request.onerror = () => { db.close(); reject(new Error('Identity read failed')); };
      request.onsuccess = () => {
        // Export only public attribution. Never include secret keys or tokens.
        const id = request.result.find((item: { pseudonymId?: string }) => item.pseudonymId)?.pseudonymId;
        db.close();
        if (!id) reject(new Error('Registered public pseudonym missing'));
        else resolve(id);
      };
    };
  }));
  return value;
}

async function read<T>(who: Actor, route: string): Promise<T> {
  await expect.poll(() => Boolean(who.headers.authorization)).toBe(true);
  const response = await who.context.request.get(new URL(route, baseURL).href, { headers: who.headers });
  expect(response.status(), `Real GET ${route}`).toBe(200);
  return response.json() as Promise<T>;
}

/** Read native measurements; keep ended sender tracks to prove capture cleanup. */
async function snapshot(who: Actor): Promise<MediaSnapshot> {
  return who.page.evaluate(async () => {
    const probe = window.__annexMediaAcceptance;
    if (!probe) throw new Error('Native media observer missing');
    const inbound: Inbound[] = [];
    const objectId = (track: MediaStreamTrack) => {
      let id = probe.trackObjects.get(track);
      if (id === undefined) { id = probe.nextTrackObjectId++; probe.trackObjects.set(track, id); }
      return id;
    };
    for (const [index, pc] of probe.peers.entries()) {
      for (const sender of pc.getSenders()) {
        if (sender.track) probe.captures.set(sender.track.id, sender.track);
      }
      if (pc.signalingState === 'closed') continue;
      const stats = await pc.getStats();
      stats.forEach((entry: RTCInboundRtpStreamStats) => {
        if (entry.type !== 'inbound-rtp') return;
        const transceiver = pc.getTransceivers().find(item => item.mid === entry.mid);
        const trackId = entry.trackIdentifier || transceiver?.receiver.track.id || '';
        const codec = entry.codecId ? stats.get(entry.codecId) as { mimeType?: string } | undefined : undefined;
        inbound.push({
          key: `${index}:${entry.id}`, trackId, kind: entry.kind,
          packets: entry.packetsReceived ?? 0, bytes: entry.bytesReceived ?? 0,
          frames: entry.framesDecoded ?? 0, samples: entry.totalSamplesReceived ?? 0,
          energy: entry.totalAudioEnergy ?? 0, width: entry.frameWidth ?? 0,
          height: entry.frameHeight ?? 0, codec: codec?.mimeType ?? '',
          receiverObjectId: transceiver ? objectId(transceiver.receiver.track) : null,
          mid: transceiver?.mid ?? null, keyFrames: entry.keyFramesDecoded ?? 0,
          framesDropped: entry.framesDropped ?? 0, jitterBufferDelay: entry.jitterBufferDelay ?? 0,
          jitterBufferEmittedCount: entry.jitterBufferEmittedCount ?? 0,
          estimatedPlayoutTimestamp: (entry as RTCInboundRtpStreamStats & { estimatedPlayoutTimestamp?: number }).estimatedPlayoutTimestamp ?? null,
        });
      });
    }
    const tracks = (element: HTMLMediaElement) => element.srcObject instanceof MediaStream
      ? element.srcObject.getTracks() : [];
    return {
      at: new Date().toISOString(),
      captureEvents: probe.captureEvents,
      receiverEvents: probe.receiverEvents,
      receivers: probe.peers.flatMap((pc, peer) => pc.getTransceivers().map(transceiver => {
        const track = transceiver.receiver.track;
        return { peer, mid: transceiver.mid, direction: transceiver.currentDirection, objectId: objectId(track),
          trackId: track.id, kind: track.kind, readyState: track.readyState, muted: track.muted, enabled: track.enabled };
      })),
      peers: probe.peers.map((pc, id) => ({ id, connection: pc.connectionState, signaling: pc.signalingState })),
      inbound,
      captures: [...probe.captures.values()].map(track => ({
        id: track.id, kind: track.kind, state: track.readyState, enabled: track.enabled,
        displaySurface: track.getSettings().displaySurface ?? null,
      })),
      videos: [...document.querySelectorAll<HTMLVideoElement>('.call-grid video')].flatMap(element =>
        tracks(element).map(track => ({ trackId: track.id, width: element.videoWidth,
          height: element.videoHeight, frames: element.getVideoPlaybackQuality().totalVideoFrames,
          paused: element.paused, time: element.currentTime, objectId: objectId(track), readyState: track.readyState,
          muted: track.muted, enabled: track.enabled, elementReadyState: element.readyState,
          elementNetworkState: element.networkState, mediaError: element.error?.code ?? null,
          streamActive: (element.srcObject as MediaStream).active,
          matchingReceivers: probe.peers.flatMap((pc, peer) => pc.getTransceivers()
            .filter(transceiver => transceiver.receiver.track === track)
            .map(transceiver => ({ peer, mid: transceiver.mid, objectId: objectId(transceiver.receiver.track) }))),
          nativeFrames: probe.videoFrames.get(element) ?? null }))),
      audio: [...document.querySelectorAll<HTMLAudioElement>('audio[data-webrtc-remote]')].flatMap(element =>
        tracks(element).map(track => ({ trackId: track.id, muted: element.muted,
          volume: element.volume, paused: element.paused, time: element.currentTime }))),
    };
  });
}

async function record(step: string, who: Actor): Promise<MediaSnapshot> {
  const measured = await snapshot(who);
  observations.push({ step, observer: who === founder ? 'founder' : who === peer ? 'peer' : 'outsider', snapshot: measured });
  return measured;
}

const trackId = (source: Source, sender: Actor) => `${source}-${sender.pseudonym}`;
function totals(measured: MediaSnapshot, id: string) {
  return measured.inbound.filter(item => item.trackId === id).reduce((sum, item) => ({
    packets: sum.packets + item.packets, bytes: sum.bytes + item.bytes,
    frames: sum.frames + item.frames, energy: sum.energy + item.energy,
  }), { packets: 0, bytes: 0, frames: 0, energy: 0 });
}

async function advancing(receiver: Actor, sender: Actor, source: Source, step: string) {
  const id = trackId(source, sender);
  // Baseline only once the attributed native receiver exists: no counting a
  // newly created entry as an interval of flowing media.
  await expect.poll(async () => (await snapshot(receiver)).inbound.some(item => item.trackId === id), {
    message: `${step}: native inbound RTP for ${source} must be attributable to the other identity`,
  }).toBe(true);
  const before = await record(`${step}:before`, receiver);
  const initial = totals(before, id);
  await expect.poll(async () => {
    const current = await snapshot(receiver);
    const total = totals(current, id);
    const transport = total.packets > initial.packets + 5 && total.bytes > initial.bytes + 100;
    if (source === 'audio') return transport && total.energy > initial.energy + 0.00001;
    const video = current.videos.find(item => item.trackId === id);
    return transport && total.frames > initial.frames + 3
      && Boolean(video && video.width > 0 && video.height > 0 && video.frames > 3 && !video.paused);
  }, { timeout: 45_000, intervals: [250, 500, 1_000], message: `${step}: independent receiver must decode changing ${source}` }).toBe(true);
  const after = await record(`${step}:after`, receiver);
  if (source !== 'audio') {
    expect(after.inbound.find(item => item.trackId === id)?.codec).toMatch(/^video\//);
  }
}

async function stopped(receiver: Actor, sender: Actor, source: Source, step: string) {
  const id = trackId(source, sender);
  // Wait for queued packets to drain, then require a complete quiet interval.
  // A vanished receiver is also valid after leave/removal. A counter reset is
  // never accepted as continued progress.
  await expect.poll(async () => {
    const before = await snapshot(receiver);
    await receiver.page.waitForTimeout(1_500);
    const after = await snapshot(receiver);
    const previous = before.inbound.filter(item => item.trackId === id);
    const current = after.inbound.filter(item => item.trackId === id);
    if (current.length === 0) return true;
    if (previous.length === 0) return false;
    return current.every(item => {
      const old = previous.find(candidate => candidate.key === item.key);
      return old && item.packets === old.packets && item.frames === old.frames && item.energy === old.energy;
    });
  }, { timeout: 30_000, intervals: [500], message: `${step}: remote ${source} must stop arriving` }).toBe(true);
  await record(step, receiver);
}

async function captureStopped(who: Actor, ids: string[], step: string) {
  expect(ids.length, `${step}: must have observed actual capture before stopping it`).toBeGreaterThan(0);
  await expect.poll(async () => {
    const measured = await snapshot(who);
    return ids.every(id => measured.captures.find(item => item.id === id)?.state === 'ended');
  }, { message: `${step}: native capture tracks must end` }).toBe(true);
  await record(step, who);
}

function channelRow(page: Page, name: string) {
  return page.locator('.channel-item').filter({ has: page.locator('.channel-name', { hasText: new RegExp(`^${name}$`) }) });
}

async function selectChannel(who: Actor, name: string) {
  const row = channelRow(who.page, name);
  await expect(row).toBeVisible();
  if (await row.locator('.join-btn').isVisible()) {
    const joined = who.page.waitForResponse(response => response.request().method() === 'POST'
      && new URL(response.url()).pathname.endsWith('/join'));
    await row.locator('.join-btn').click();
    expect((await joined).ok()).toBe(true);
  }
  await row.locator('.channel-select').click();
  await expect(row).toHaveClass(/active/);
}

async function joinCall(who: Actor, channel: Channel) {
  const joined = who.page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === `/api/channels/${channel.channel_id}/voice/join`);
  await expect(who.page.locator('.voice-join-btn')).toBeEnabled();
  await who.page.locator('.voice-join-btn').click();
  expect((await joined).ok()).toBe(true);
  await expect(who.page.locator('.media-controls')).toBeVisible();
  await expect.poll(async () => (await snapshot(who)).peers.some(pc => pc.connection === 'connected')).toBe(true);
  await expect(who.page.locator('.media-controls').getByTitle('Mute microphone', { exact: true })).toBeVisible();
}

async function screenshot(name: string, who: Actor) {
  const path = test.info().outputPath(`${name}.png`);
  await who.page.screenshot({ path });
  await test.info().attach(name, { path, contentType: 'image/png' });
}

test.afterEach(async () => {
  for (const who of actors) {
    if (!who.page.isClosed()) await record('final native state', who).catch(() => {});
  }
  await test.info().attach('native-media-observations', {
    body: JSON.stringify({
      mode,
      boundary: mode === 'controlled'
        ? 'Piper WAV via Chromium controlled microphone; Chromium generated camera with software rendering (--disable-gpu); actual captured browser tab; real SFU and remote decoders. Does not prove GPU rendering or physical microphone/camera/speaker hardware.'
        : 'Physical microphone and camera; actual captured browser tab; real SFU and remote decoders. Speaker audibility still needs human confirmation.',
      observations,
      captions: actors.map(who => ({ recipient: who.pseudonym, received: who.captions })),
      consoleErrors: actors.map(who => ({ recipient: who.pseudonym, messages: who.consoleErrors })),
    }, null, 2), contentType: 'application/json',
  });
  for (const who of actors) await who.context.close();
});

test(`two independent peers exchange and stop native media (${mode} input)`, async ({ browser }) => {
  founder = await actor(browser);
  await expect(founder.page.locator('.admin-menu-btn'), 'This lane requires a fresh server database').toBeVisible();
  peer = await actor(browser);
  outsider = await actor(browser);
  expect(founder.pseudonym).not.toBe(peer.pseudonym);
  const readiness = await read<{ voice_enabled: boolean; infrastructure_ready: boolean; stt_ready?: boolean }>(founder, '/api/voice/config-status');
  expect(readiness.voice_enabled).toBe(true);
  expect(readiness.infrastructure_ready).toBe(true);

  const screen = await founder.context.newPage();
  await screen.setContent(`<!doctype html><title>Annex controlled screen acceptance</title>
    <style>html,body{margin:0;width:100%;height:100%;background:#173f67;color:white;font:32px sans-serif}
    main{padding:50px}i{display:block;width:150px;height:150px;background:#ffc800;animation:move 2s linear infinite alternate}
    @keyframes move{to{transform:translateX(500px) rotate(180deg);background:#00df9a}}</style>
    <main>Annex controlled screen acceptance — dedicated capture surface<i></i><p id="clock"></p></main>
    <script>setInterval(()=>document.getElementById('clock').textContent=new Date().toISOString(),100)</script>`);
  const name = `media-${Date.now()}`;
  await founder.page.locator('.create-channel-btn').click();
  const dialog = founder.page.getByRole('dialog');
  await dialog.getByPlaceholder('general').fill(name);
  await dialog.locator('select').selectOption('Hybrid');
  const created = founder.page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === '/api/channels');
  await dialog.getByRole('button', { name: 'Create', exact: true }).click();
  expect((await created).status()).toBe(200);
  await expect(dialog).toBeHidden();
  const channel = (await read<Channel[]>(founder, '/api/channels')).find(item => item.name === name)!;
  expect(channel.channel_type).toBe('Hybrid');
  await selectChannel(founder, name);
  await selectChannel(peer, name);
  await joinCall(founder, channel);
  await joinCall(peer, channel);

  await test.step('both independent receivers decode speech audio', async () => {
    await advancing(peer, founder, 'audio', 'founder audio received by peer');
    await advancing(founder, peer, 'audio', 'peer audio received by founder');
    const status = await read<{ participant_ids: string[] }>(founder, `/api/channels/${channel.channel_id}/voice/status`);
    expect(status.participant_ids.sort()).toEqual([founder.pseudonym, peer.pseudonym].sort());
  });

  await test.step('mute ends capture and stops remote audio; unmute restores reception', async () => {
    const ids = (await record('before mute', founder)).captures.filter(item => item.kind === 'audio' && item.state === 'live').map(item => item.id);
    await founder.page.locator('.media-controls').getByTitle('Mute microphone', { exact: true }).click();
    await captureStopped(founder, ids, 'muted microphone ended');
    await stopped(peer, founder, 'audio', 'muted founder audio stopped at peer');
    await advancing(founder, peer, 'audio', 'opposite audio direction survives mute');
    await founder.page.locator('.media-controls').getByTitle('Unmute microphone', { exact: true }).click();
    await advancing(peer, founder, 'audio', 'unmuted founder audio received');
  });

  await test.step('deafen changes actual remote audio playback elements', async () => {
    await peer.page.getByRole('region', { name: 'Voice call status', exact: true }).getByTitle('Deafen — mute all incoming audio', { exact: true }).click();
    await expect.poll(async () => {
      const audio = (await snapshot(peer)).audio.filter(item => item.trackId === trackId('audio', founder));
      return audio.length > 0 && audio.every(item => item.muted && item.volume === 0);
    }).toBe(true);
    await record('peer deafened playback', peer);
    await peer.page.getByRole('region', { name: 'Voice call status', exact: true }).getByTitle('Undeafen — resume hearing others', { exact: true }).click();
    await expect.poll(async () => {
      const audio = (await snapshot(peer)).audio.filter(item => item.trackId === trackId('audio', founder));
      return audio.length > 0 && audio.every(item => !item.muted && item.volume > 0 && !item.paused);
    }).toBe(true);
    await advancing(peer, founder, 'audio', 'undeafened playback and audio reception');
  });

  await test.step('camera and screen arrive as separately decoded sources', async () => {
    await founder.page.locator('.media-controls').getByTitle('Turn on camera', { exact: true }).click();
    await advancing(peer, founder, 'video', 'founder camera decoded');
    await peer.page.locator('.media-controls').getByTitle('Turn on camera', { exact: true }).click();
    await advancing(founder, peer, 'video', 'peer camera decoded');
    await founder.page.locator('.media-controls').getByTitle('Share screen', { exact: true }).click();
    await expect(founder.page.locator('.media-controls').getByTitle('Stop sharing', { exact: true })).toBeVisible();
    const capture = await record('dedicated tab capture started', founder);
    expect(capture.captures.some(item => item.state === 'live' && item.displaySurface === 'browser'),
      'Screen source must be the dedicated real browser tab, never the physical desktop').toBe(true);
    await advancing(peer, founder, 'screen', 'founder screen decoded separately');
    await advancing(peer, founder, 'video', 'camera continues beside screen');
    await screenshot('independent-peer-camera-and-screen', peer);
  });

  await test.step('camera off preserves screen, screen off stops its own source', async () => {
    const before = await record('before camera off', founder);
    const cameraIds = before.captures.filter(item => item.kind === 'video' && item.state === 'live' && !item.displaySurface).map(item => item.id);
    await founder.page.locator('.media-controls').getByTitle('Turn off camera', { exact: true }).click();
    await captureStopped(founder, cameraIds, 'camera capture ended');
    await stopped(peer, founder, 'video', 'camera packets stopped remotely');
    await advancing(peer, founder, 'screen', 'screen survives camera off');
    const screenIds = (await snapshot(founder)).captures.filter(item => item.displaySurface && item.state === 'live').map(item => item.id);
    await founder.page.locator('.media-controls').getByTitle('Stop sharing', { exact: true }).click();
    await captureStopped(founder, screenIds, 'screen capture ended');
    await stopped(peer, founder, 'screen', 'screen packets stopped remotely');
    await advancing(peer, founder, 'audio', 'audio survives both video sources off');
  });

  await test.step('leave closes native media and rejoin restores both directions', async () => {
    const ids = (await record('before leaving', founder)).captures.filter(item => item.state === 'live').map(item => item.id);
    await founder.page.locator('.media-controls').getByTitle('Leave call', { exact: true }).click();
    await expect(founder.page.locator('.media-controls')).toHaveCount(0);
    await captureStopped(founder, ids, 'leave ended all local capture');
    await expect.poll(async () => (await snapshot(founder)).peers.every(item => item.signaling === 'closed')).toBe(true);
    await stopped(peer, founder, 'audio', 'leave stopped remote audio');
    await expect.poll(async () => (await read<{ participant_ids: string[] }>(peer,
      `/api/channels/${channel.channel_id}/voice/status`)).participant_ids).toEqual([peer.pseudonym]);
    await joinCall(founder, channel);
    await advancing(peer, founder, 'audio', 'rejoined founder audio');
    await advancing(founder, peer, 'audio', 'rejoined founder hears peer');
    await founder.page.locator('.media-controls').getByTitle('Turn on camera', { exact: true }).click();
    await advancing(peer, founder, 'video', 'rejoined camera decoded');
    await founder.page.locator('.media-controls').getByTitle('Share screen', { exact: true }).click();
    await advancing(peer, founder, 'screen', 'rejoined screen decoded');
  });

  await test.step('both human peers receive captions from the real speech pipeline', async () => {
    expect(readiness.stt_ready).toBe(true);
    const phrase = process.env.ANNEX_EXPECTED_CAPTION ?? 'Annex Live Session Speech Verification';
    for (const who of [founder, peer]) {
      const speaker = who === founder ? peer : founder;
      // STT retains six seconds of context and flushes on speaker inactivity;
      // a sentence may still span several visible transcript lines.
      // Keep attribution to the other real participant while joining windows.
      const lines = who.page.locator('.voice-caption').filter({
        has: who.page.locator('.voice-caption-speaker', { hasText: speaker.pseudonym.slice(0, 14) }),
      }).locator('.voice-caption-text');
      await expect.poll(async () => (await lines.allTextContents()).join(' ')
        .replace(/[^\p{L}\p{N}]+/gu, ' ').trim(), {
        timeout: 120_000, message: 'Recognized peer speech must reach visible captions with correct attribution',
      }).toMatch(new RegExp(phrase, 'i'));
      expect(who.captions.some(item => item.channelId === channel.channel_id
        && item.speakerPseudonym === speaker.pseudonym)).toBe(true);
    }
    expect(outsider.captions, 'A registered nonparticipant must receive no room transcripts').toEqual([]);
    await screenshot('real-speech-captions-at-peer', peer);
  });

  await test.step('deleting an active channel ends capture and remote transport for both peers', async () => {
    const localIds = (await record('before channel deletion', founder)).captures.filter(item => item.state === 'live').map(item => item.id);
    const peerIds = (await record('before channel deletion', peer)).captures.filter(item => item.state === 'live').map(item => item.id);
    await founder.page.locator('.admin-menu-btn').click();
    await founder.page.getByRole('button', { name: 'Channel Management', exact: true }).click();
    await founder.page.locator('.channel-manager-item', { hasText: name }).getByRole('button', { name: 'Delete', exact: true }).click();
    // Keep the destructive trigger causal: navigating to administration or
    // opening its confirmation must not already have stopped the call.
    for (const source of ['audio', 'video', 'screen'] as const) {
      await advancing(peer, founder, source, `${source} remains live while deletion awaits confirmation`);
    }
    const deleted = founder.page.waitForResponse(response => response.request().method() === 'DELETE'
      && new URL(response.url()).pathname === `/api/channels/${channel.channel_id}`);
    await founder.page.getByRole('dialog').getByRole('button', { name: 'Delete channel', exact: true }).click();
    expect((await deleted).ok()).toBe(true);
    await expect(channelRow(peer.page, name)).toHaveCount(0);
    await captureStopped(founder, localIds, 'channel deletion ended founder capture');
    await captureStopped(peer, peerIds, 'channel deletion ended peer capture');
    for (const who of [founder, peer]) {
      await expect.poll(async () => (await snapshot(who)).peers.every(item => item.signaling === 'closed')).toBe(true);
      await expect(who.page.locator('.media-controls')).toHaveCount(0);
      await expect(who.page.locator('audio[data-webrtc-remote]')).toHaveCount(0);
      await expect(who.page.locator('.voice-caption')).toHaveCount(0);
    }
    for (const source of ['audio', 'video', 'screen'] as const) await stopped(peer, founder, source, `deletion stopped ${source}`);
    expect((await read<Channel[]>(founder, '/api/channels')).some(item => item.channel_id === channel.channel_id)).toBe(false);
  });
  for (const who of actors) expect(who.errors, 'Uncaught application errors').toEqual([]);
  expect(outsider.captions, 'Nonparticipants must still receive no captions during room cleanup').toEqual([]);
});
