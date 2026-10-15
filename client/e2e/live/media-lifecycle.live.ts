import { test, expect, type Browser, type BrowserContext, type Page } from '@playwright/test';
import { completeStartup } from '../helpers';
import { publicIdentities } from './operator-helpers';
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
declare global { interface Window { __annexMediaLifecycleAcceptance?: Probe } }
interface Actor {
  context: BrowserContext;
  page: Page;
  socketAttempts: number;
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

async function actor(browser: Browser, register = true): Promise<Actor> {
  const context = await browser.newContext({ baseURL, permissions: ['microphone', 'camera'] });
  await context.addInitScript(() => {
    const probe: Probe = { peers: [], captures: new Map(), captureEvents: [], receiverEvents: [],
      trackObjects: new WeakMap(), nextTrackObjectId: 1, videoFrames: new WeakMap() };
    window.__annexMediaLifecycleAcceptance = probe;
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
    const nativeDisplay = navigator.mediaDevices.getDisplayMedia.bind(navigator.mediaDevices);
    navigator.mediaDevices.getDisplayMedia = async options => {
      const stream = await nativeDisplay(options);
      for (const track of stream.getTracks()) {
        probe.captures.set(track.id, track);
        probe.captureEvents.push({ event: 'display-resolved', id: track.id, kind: track.kind, at: Date.now() });
        track.addEventListener('ended', () => probe.captureEvents.push({ event: 'native-ended', id: track.id, kind: track.kind, at: Date.now() }));
      }
      return stream;
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
  const value: Actor = { context, page, headers: {}, socketAttempts: 0, pseudonym: '', errors: [], consoleErrors: [], captions: [] };
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
  page.on('websocket', () => { value.socketAttempts++; });
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
  if (!register) return value;
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
    const probe = window.__annexMediaLifecycleAcceptance;
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
  observations.push({ step, observer: who === founder ? 'founder' : who === peer ? 'peer' : 'linked-device', snapshot: measured });
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
  await test.info().attach('native-media-lifecycle-observations', {
    body: JSON.stringify({
      mode,
      boundary: mode === 'controlled'
        ? 'Piper WAV via Chromium controlled microphone; Chromium generated camera with software rendering (--disable-gpu); actual captured browser tab; real SFU and remote decoders. Does not prove GPU rendering or physical microphone/camera/speaker hardware.'
        : 'Physical microphone and camera; actual captured browser tab; real SFU and remote decoders. Speaker audibility still needs human confirmation.',
      observations,
      captions: actors.map(who => ({ recipient: who.pseudonym, received: who.captions })),
      socketAttempts: actors.map(who => ({ pseudonym: who.pseudonym, attempts: who.socketAttempts })),
      consoleErrors: actors.map(who => ({ recipient: who.pseudonym, messages: who.consoleErrors })),
    }, null, 2), contentType: 'application/json',
  });
  for (const who of actors) await who.context.close();
});


async function makeScreen(who: Actor) {
  const surface = await who.context.newPage();
  await surface.setContent(`<!doctype html><title>Annex controlled screen acceptance</title>
    <style>body{background:#173f67;color:white;font:32px sans-serif}i{display:block;width:150px;height:150px;background:#ffc800;animation:move 2s linear infinite alternate}@keyframes move{to{transform:translateX(500px) rotate(180deg);background:#00df9a}}</style>
    <main>Dedicated Annex capture surface<i></i><p id="clock"></p></main>
    <script>setInterval(()=>document.getElementById('clock').textContent=new Date().toISOString(),100)</script>`);
  return surface;
}

async function publicPseudonym(who: Actor): Promise<string> {
  const identities = await publicIdentities(who);
  const pseudonym = identities.find(item => item.pseudonymId)?.pseudonymId;
  expect(pseudonym).toBeTruthy();
  return pseudonym!;
}

async function actualPlaybackVolume(who: Actor, sender: Actor, volume: number, muted: boolean) {
  const audio = (await snapshot(who)).audio.filter(value => value.trackId === trackId('audio', sender));
  return audio.length > 0 && audio.every(value => value.volume === volume && value.muted === muted && !value.paused);
}

async function enableVideo(who: Actor, receiver: Actor) {
  await who.page.locator('.media-controls').getByTitle('Turn on camera', { exact: true }).click();
  await advancing(receiver, who, 'video', 'camera decoded before lifecycle action');
  await who.page.locator('.media-controls').getByTitle('Share screen', { exact: true }).click();
  await advancing(receiver, who, 'screen', 'dedicated tab decoded before lifecycle action');
  expect((await snapshot(who)).captures.some(item => item.state === 'live' && item.displaySurface === 'browser')).toBe(true);
}

async function allSourcesStopped(who: Actor, receiver: Actor, ids: string[], step: string) {
  await captureStopped(who, ids, `${step}: all captured tracks ended`);
  await expect(who.page.locator('.media-controls')).toHaveCount(0);
  await expect(who.page.locator('audio[data-webrtc-remote]')).toHaveCount(0);
  await expect.poll(async () => (await snapshot(who)).peers.every(pc => pc.signaling === 'closed')).toBe(true);
  for (const source of ['audio', 'video', 'screen'] as const) await stopped(receiver, who, source, `${step}: ${source} stopped at independent receiver`);
}

async function sendText(who: Actor, receiver: Actor, channel: Channel, text: string) {
  const composer = who.page.getByPlaceholder('Type a message...', { exact: true });
  await composer.fill(text);
  await who.page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(receiver.page.locator('.message-content', { hasText: text })).toBeVisible();
  await expect.poll(async () => (await read<Array<{ content: string; sender_pseudonym: string }>>(receiver,
    `/api/channels/${channel.channel_id}/messages?limit=50`)).some(message => message.content === text && message.sender_pseudonym === who.pseudonym)).toBe(true);
}

async function setHumanVoice(moderator: Actor, target: Actor, enabled: boolean) {
  await moderator.page.locator('.admin-menu-btn').click();
  await moderator.page.getByRole('button', { name: 'Member Management', exact: true }).click();
  const row = moderator.page.locator('.member-row').filter({ has: moderator.page.locator(`.member-pseudonym[title="${target.pseudonym}"]`) });
  const checkbox = row.getByRole('checkbox', { name: 'Voice', exact: true });
  await expect(checkbox).toBeChecked({ checked: !enabled });
  const response = moderator.page.waitForResponse(value => value.request().method() === 'PATCH'
    && new URL(value.url()).pathname === `/api/admin/members/${target.pseudonym}/capabilities`);
  // This controlled checkbox commits only after the actual server accepts the
  // capability change. A click may finish before that asynchronous response.
  await checkbox.click();
  expect((await response).status()).toBe(200);
  await expect(checkbox).toBeChecked({ checked: enabled });
  const persisted = await read<{ members: Array<{ pseudonym_id: string; participant_type: string; can_voice: boolean }> }>(moderator, '/api/admin/members');
  const member = persisted.members.find(value => value.pseudonym_id === target.pseudonym)!;
  expect(member.participant_type.toUpperCase()).toBe('HUMAN');
  expect(member.can_voice).toBe(enabled);
  await moderator.page.getByRole('button', { name: 'Chat', exact: true }).click();
}

async function linkDuringCall(source: Actor, target: Actor) {
  await source.page.getByRole('button', { name: 'Link', exact: true }).click();
  await source.page.getByRole('button', { name: /Link a New Device/ }).click();
  // The encrypted code and its pairing secret exist only in process memory.
  // Independent QR decoding is qualified in the separate device-link lane.
  const transfer = await source.page.getByLabel('Transfer code', { exact: true }).inputValue();
  const pairing = (await source.page.locator('.pairing-code-value').innerText()).trim();
  await target.page.goto('/');
  await target.page.getByRole('button', { name: 'Link from Another Device', exact: true }).click();
  await target.page.getByRole('button', { name: /Receive from Another Device/ }).click();
  await target.page.getByPlaceholder('Paste the encoded data here...').fill(transfer);
  await target.page.getByPlaceholder('000000').fill(pairing);
  await target.page.getByRole('button', { name: 'Import Identity', exact: true }).click();
  await expect.poll(async () => (await publicIdentities(target)).length).toBe(1);
  const continued = target.page.getByRole('dialog').getByRole('button', { name: 'Continue', exact: true });
  if (await continued.isVisible()) await continued.click();
  await source.page.getByRole('dialog').getByRole('button', { name: 'Done', exact: true }).click();
  await completeStartup(target.page);
  target.pseudonym = await publicPseudonym(target);
  expect(target.pseudonym).toBe(source.pseudonym);
}

test(`human call controls, native devices, revocation and active-device transfer (${mode})`, async ({ browser }) => {
  founder = await actor(browser);
  await expect(founder.page.locator('.admin-menu-btn'), 'Fresh database required').toBeVisible();
  peer = await actor(browser);
  const name = `media-lifecycle-${Date.now()}`;
  await founder.page.locator('.create-channel-btn').click();
  const create = founder.page.getByRole('dialog');
  await create.getByPlaceholder('general').fill(name);
  await create.locator('select').selectOption('Hybrid');
  const created = founder.page.waitForResponse(response => response.request().method() === 'POST' && new URL(response.url()).pathname === '/api/channels');
  await create.getByRole('button', { name: 'Create', exact: true }).click();
  expect((await created).status()).toBe(200);
  const channel = (await read<Channel[]>(founder, '/api/channels')).find(value => value.name === name)!;
  await selectChannel(founder, name);
  await selectChannel(peer, name);
  await joinCall(founder, channel);
  await joinCall(peer, channel);
  await advancing(founder, peer, 'audio', 'initial peer speech');
  await advancing(peer, founder, 'audio', 'initial founder speech');
  let screen = await makeScreen(peer);
  await enableVideo(peer, founder);

  await test.step('status-bar microphone controls work while another channel is visible', async () => {
    await selectChannel(peer, 'General');
    const strip = peer.page.getByRole('region', { name: 'Voice call status', exact: true });
    await expect(strip).toContainText(name);
    await advancing(founder, peer, 'video', 'camera survives navigation');
    await advancing(founder, peer, 'screen', 'screen survives navigation');
    const ids = (await snapshot(peer)).captures.filter(value => value.kind === 'audio' && value.state === 'live').map(value => value.id);
    await strip.getByTitle('Mute microphone', { exact: true }).click();
    await captureStopped(peer, ids, 'status microphone mute');
    await stopped(founder, peer, 'audio', 'status microphone mute at receiver');
    await expect(peer.page.locator('.media-controls').getByTitle('Unmute microphone', { exact: true })).toBeVisible();
    await advancing(founder, peer, 'video', 'status microphone mute preserves camera');
    await strip.getByTitle('Unmute microphone', { exact: true }).click();
    await advancing(founder, peer, 'audio', 'status microphone unmute');
    await selectChannel(peer, name);
  });

  await test.step('live device selections and output volume reach native capture and playback', async () => {
    await peer.page.getByRole('button', { name: /^Audio & Video Settings/ }).click();
    const settings = peer.page.getByRole('dialog', { name: 'Audio & Video Settings', exact: true });
    for (const [label, kind] of [['Input Device', 'audio'], ['Camera', 'video']] as const) {
      const select = settings.getByRole('combobox', { name: new RegExp(`^${label}`) });
      await expect.poll(() => select.locator('option').evaluateAll(options => options.some(item => (item as HTMLOptionElement).value !== '')),
        { message: `${label}: wait for actual native device enumeration` }).toBe(true);
      const option = await select.locator('option').evaluateAll(options => options.map(item => (item as HTMLOptionElement).value).find(value => value !== ''));
      expect(option, `${label}: a real enumerated device is required`).toBeTruthy();
      const old = (await snapshot(peer)).captures.filter(value => value.kind === kind && value.state === 'live' && !value.displaySurface).map(value => value.id);
      await select.selectOption(option!);
      await expect.poll(() => peer.page.evaluate(({ kind, selected }) => window.__annexMediaLifecycleAcceptance!.peers.some(pc =>
        pc.getSenders().some(sender => sender.track?.kind === kind && sender.track.readyState === 'live'
          && !sender.track.getSettings().displaySurface && sender.track.getSettings().deviceId === selected)), { kind, selected: option! })).toBe(true);
      await captureStopped(peer, old, `${label}: previous native capture ended after replacement`);
      await advancing(founder, peer, kind === 'audio' ? 'audio' : 'video', `${label}: replacement actually decoded`);
      const replacement = (await snapshot(peer)).captures.filter(value => value.kind === kind && value.state === 'live' && !value.displaySurface).map(value => value.id);
      await select.selectOption('');
      await captureStopped(peer, replacement, `${label}: returning to system default replaces old capture`);
      await advancing(founder, peer, kind === 'audio' ? 'audio' : 'video', `${label}: default decoded`);
    }
    const output = settings.getByRole('combobox', { name: /^Output Device/ });
    await expect(output, 'Chromium native output routing must be available for this lane').toBeEnabled();
    await expect.poll(() => output.locator('option').evaluateAll(options => options.some(item => (item as HTMLOptionElement).value !== '')),
      { message: 'Wait for actual native output enumeration' }).toBe(true);
    const sink = await output.locator('option').evaluateAll(options => options.map(item => (item as HTMLOptionElement).value).find(value => value !== ''));
    expect(sink, 'An actual enumerated output is required').toBeTruthy();
    await output.selectOption(sink!);
    await expect.poll(() => peer.page.locator('audio[data-webrtc-remote]').evaluateAll((elements, sink) => elements.length > 0
      && elements.every(element => (element as HTMLAudioElement).sinkId === sink), sink!)).toBe(true);
    const volume = settings.getByRole('slider');
    await volume.focus(); await volume.press('Home');
    for (let count = 0; count < 37; count++) await volume.press('ArrowRight');
    await expect.poll(() => actualPlaybackVolume(peer, founder, 0.37, false)).toBe(true);
    await settings.getByRole('button', { name: 'Done', exact: true }).click();
    const strip = peer.page.getByRole('region', { name: 'Voice call status', exact: true });
    await strip.getByTitle('Deafen — mute all incoming audio', { exact: true }).click();
    await expect.poll(() => actualPlaybackVolume(peer, founder, 0, true)).toBe(true);
    await strip.getByTitle('Undeafen — resume hearing others', { exact: true }).click();
    await expect.poll(() => actualPlaybackVolume(peer, founder, 0.37, false)).toBe(true);
    await peer.page.getByRole('button', { name: /^Audio & Video Settings/ }).click();
    await settings.getByRole('combobox', { name: /^Output Device/ }).selectOption('');
    await expect.poll(() => peer.page.locator('audio[data-webrtc-remote]').evaluateAll(elements => elements.length > 0
      && elements.every(element => (element as HTMLAudioElement).sinkId === ''))).toBe(true);
    await settings.getByRole('slider').focus(); await settings.getByRole('slider').press('End');
    await settings.getByRole('button', { name: 'Done', exact: true }).click();
    await record('actual device selections and restored output', peer);
  });

  await test.step('closing the captured tab ends only screen sharing and ordinary Share screen restarts it', async () => {
    const ids = (await snapshot(peer)).captures.filter(value => value.state === 'live' && value.displaySurface).map(value => value.id);
    await screen.close();
    await captureStopped(peer, ids, 'actual native tab closure');
    await expect(peer.page.locator('.media-controls').getByTitle('Share screen', { exact: true })).toBeVisible();
    await stopped(founder, peer, 'screen', 'native ended display source');
    await advancing(founder, peer, 'audio', 'audio survives native display end');
    await advancing(founder, peer, 'video', 'camera survives native display end');
    screen = await makeScreen(peer);
    await peer.page.locator('.media-controls').getByTitle('Share screen', { exact: true }).click();
    await advancing(founder, peer, 'screen', 'new actual tab capture decoded');
  });

  await test.step('status-bar disconnect ends all sources and a new call needs explicit Join', async () => {
    const ids = (await snapshot(peer)).captures.filter(value => value.state === 'live').map(value => value.id);
    await peer.page.getByRole('region', { name: 'Voice call status', exact: true }).getByTitle('Disconnect from voice channel', { exact: true }).click();
    await allSourcesStopped(peer, founder, ids, 'status-bar disconnect');
    await expect.poll(async () => (await read<{ participant_ids: string[] }>(founder, `/api/channels/${channel.channel_id}/voice/status`)).participant_ids).toEqual([founder.pseudonym]);
    await joinCall(peer, channel);
    await advancing(founder, peer, 'audio', 'explicit rejoin audio');
    await enableVideo(peer, founder);
  });

  await test.step('linked-device takeover closes an active call and both displaced devices stay quiet', async () => {
    const loaded = `readable-before-takeover-${Date.now()}`;
    await sendText(peer, founder, channel, loaded);
    const ids = (await snapshot(peer)).captures.filter(value => value.state === 'live').map(value => value.id);
    outsider = await actor(browser, false);
    await linkDuringCall(peer, outsider);
    await expect(peer.page.getByText('This identity is active on another device.', { exact: true })).toBeVisible();
    await allSourcesStopped(peer, founder, ids, 'active-call takeover');
    await expect(peer.page.locator('.message-content', { hasText: loaded })).toBeVisible();
    await expect(peer.page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
    const before = peer.socketAttempts;
    await peer.page.waitForTimeout(31_500);
    expect(peer.socketAttempts, 'No automatic ownership transfer during maximum reconnect interval').toBe(before);
    await selectChannel(outsider, name);
    await joinCall(outsider, channel);
    await advancing(founder, outsider, 'audio', 'linked active device audio');
    // Close the old source tab so auto-selection has exactly one eligible tab.
    await screen.close(); screen = await makeScreen(outsider);
    await enableVideo(outsider, founder);
    const linkedIds = (await snapshot(outsider)).captures.filter(value => value.state === 'live').map(value => value.id);
    await peer.page.getByRole('button', { name: 'Reconnect here', exact: true }).click();
    await expect(peer.page.locator('.ws-indicator')).toHaveClass(/\bconnected\b/);
    await expect(outsider.page.getByText('This identity is active on another device.', { exact: true })).toBeVisible();
    await allSourcesStopped(outsider, founder, linkedIds, 'explicit session transfer back');
    const attempts = outsider.socketAttempts;
    await outsider.page.waitForTimeout(31_500);
    expect(outsider.socketAttempts).toBe(attempts);
    await expect(peer.page.locator('.media-controls')).toHaveCount(0);
    await screen.close(); screen = await makeScreen(peer);
    await joinCall(peer, channel);
    await advancing(founder, peer, 'audio', 'returned owner explicitly rejoined');
    await enableVideo(peer, founder);
  });

  await test.step('revoking a human voice capability ends live audio, camera and display capture while text remains authorized', async () => {
    const ids = (await snapshot(peer)).captures.filter(value => value.state === 'live').map(value => value.id);
    await setHumanVoice(founder, peer, false);
    await allSourcesStopped(peer, founder, ids, 'human capability revoked');
    await expect(peer.page.locator('.voice-join-btn')).toBeDisabled();
    await expect(peer.page.locator('.voice-panel')).toContainText(/voice.*(not allowed|disabled|revoked)/i);
    await expect.poll(async () => (await read<{ participant_ids: string[] }>(founder, `/api/channels/${channel.channel_id}/voice/status`)).participant_ids).toEqual([founder.pseudonym]);
    await expect(peer.page.locator('.ws-indicator')).toHaveClass(/\bconnected\b/);
    await sendText(peer, founder, channel, `text-authorized-with-voice-revoked-${Date.now()}`);
    await setHumanVoice(founder, peer, true);
    await expect(peer.page.locator('.voice-join-btn')).toBeEnabled();
    await expect(peer.page.locator('.media-controls')).toHaveCount(0);
    expect((await snapshot(peer)).captures.every(value => value.state === 'ended')).toBe(true);
    await joinCall(peer, channel);
    await advancing(founder, peer, 'audio', 'restored capability explicitly joins');
    await enableVideo(peer, founder);
  });

  await test.step('Logout stops all native sources and resuming the same identity does not reopen capture', async () => {
    const ids = (await snapshot(peer)).captures.filter(value => value.state === 'live').map(value => value.id);
    await peer.page.getByRole('button', { name: 'Logout', exact: true }).click();
    await expect(peer.page.getByRole('heading', { name: 'Existing Identities', exact: true })).toBeVisible();
    await allSourcesStopped(peer, founder, ids, 'logout');
    await peer.page.locator('.identity-option').filter({ hasText: peer.pseudonym.slice(0, 16) }).click();
    await completeStartup(peer.page);
    await selectChannel(peer, name);
    expect(await publicPseudonym(peer)).toBe(peer.pseudonym);
    await expect(peer.page.locator('.media-controls')).toHaveCount(0);
    await joinCall(peer, channel);
    await advancing(founder, peer, 'audio', 'resumed identity explicitly joins');
  });
  await screenshot('human-media-lifecycle-complete', founder);
  for (const who of actors) expect(who.errors, 'No uncaught application errors').toEqual([]);
});
