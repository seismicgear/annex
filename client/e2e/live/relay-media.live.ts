import { expect, test, type Browser, type Page } from '@playwright/test';
import { execFileSync } from 'node:child_process';
import { completeStartup } from '../helpers';
import { baseURL, newActor, publicIdentities, read, reload, select, type Actor } from './operator-helpers';
import type { Channel } from '../../src/types';

interface TransportEvent { at: number; peer: number; event: string; connection: string; ice: string }
interface TurnProbe { peers: RTCPeerConnection[]; captures: MediaStreamTrack[]; events: TransportEvent[] }
declare global { interface Window { __annexTurnProbe?: TurnProbe } }
type Participant = Actor & { pseudonym: string };
interface Pair { peer: number; local: string; remote: string; state: string; bytesSent: number; bytesReceived: number }
interface Incoming { peer: number; id: string; track: string; bytes: number; packets: number; samples: number; energy: number; frames: number; codec: string }
interface Snapshot { at: number; peers: Array<{ connection: string; signaling: string }>; pairs: Pair[]; incoming: Incoming[];
  events: TransportEvent[]; captures: Array<{ id: string; kind: string; state: string }>; videos: Array<{ track: string; width: number; height: number; frames: number; paused: boolean }> }
const observations: Array<{ action: string; observer: string; value: Snapshot }> = [];
const controls: unknown[] = [];
const actors: Participant[] = [];

// Only the isolated OS processes/configuration are changed. Browser ICE policy,
// returned media, signaling and application responses are never substituted.
function control(action: string) {
  const script = process.env.ANNEX_TURN_CONTROL;
  expect(script, 'Provide the actual isolated Ubuntu TURN-session controller').toBeTruthy();
  const receipt = JSON.parse(execFileSync('wsl.exe', ['-d', 'Ubuntu', '-u', 'root', '--exec', 'python3', script!, action],
    { encoding: 'utf8', timeout: 90_000, windowsHide: true }).trim());
  expect(receipt.ok, `Actual OS action ${action}`).toBe(true);
  controls.push({ action, receipt }); return receipt;
}
async function participant(browser: Browser): Promise<Participant> {
  const who = await newActor(browser, false);
  await who.context.grantPermissions(['microphone', 'camera']);
  await who.context.addInitScript(() => {
    const probe: TurnProbe = { peers: [], captures: [], events: [] }; window.__annexTurnProbe = probe;
    const NativePeer = window.RTCPeerConnection;
    window.RTCPeerConnection = class extends NativePeer {
      constructor(configuration?: RTCConfiguration) {
        super(configuration); const peer = probe.peers.length; probe.peers.push(this);
        for (const event of ['connectionstatechange', 'iceconnectionstatechange']) this.addEventListener(event, () => {
          probe.events.push({ at: Date.now(), peer, event, connection: this.connectionState, ice: this.iceConnectionState });
        });
      }
    };
    const capture = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
    navigator.mediaDevices.getUserMedia = async options => {
      const stream = await capture(options); probe.captures.push(...stream.getTracks()); return stream;
    };
    const display = navigator.mediaDevices.getDisplayMedia.bind(navigator.mediaDevices);
    navigator.mediaDevices.getDisplayMedia = async options => {
      const stream = await display(options); probe.captures.push(...stream.getTracks()); return stream;
    };
  });
  await completeStartup(who.page);
  const identities = await publicIdentities(who);
  const pseudonym = identities.find(value => value.pseudonymId)?.pseudonymId;
  expect(pseudonym).toBeTruthy();
  const actor = { ...who, pseudonym: pseudonym! }; actors.push(actor); return actor;
}
async function snapshot(page: Page): Promise<Snapshot> {
  return page.evaluate(async () => {
    const probe = window.__annexTurnProbe;
    if (!probe) throw new Error('Native media observation was not installed');
    const pairs: Pair[] = [], incoming: Incoming[] = [];
    for (const [peer, connection] of probe.peers.entries()) {
      const reports = await connection.getStats();
      reports.forEach(report => {
        if (report.type === 'transport' && report.selectedCandidatePairId) {
          const pair = reports.get(report.selectedCandidatePairId);
          if (pair) pairs.push({ peer, local: reports.get(pair.localCandidateId)?.candidateType ?? '',
            remote: reports.get(pair.remoteCandidateId)?.candidateType ?? '', state: pair.state,
            bytesSent: pair.bytesSent ?? 0, bytesReceived: pair.bytesReceived ?? 0 });
        }
        if (report.type === 'inbound-rtp') incoming.push({ peer, id: report.id,
          track: report.trackIdentifier ?? connection.getTransceivers().find(item => item.mid === report.mid)?.receiver.track.id ?? '',
          bytes: report.bytesReceived ?? 0, packets: report.packetsReceived ?? 0, samples: report.totalSamplesReceived ?? 0,
          energy: report.totalAudioEnergy ?? 0, frames: report.framesDecoded ?? 0,
          codec: reports.get(report.codecId)?.mimeType ?? '' });
      });
    }
    return { at: Date.now(), peers: probe.peers.map(pc => ({ connection: pc.connectionState, signaling: pc.signalingState })),
      pairs, incoming, events: probe.events, captures: probe.captures.map(track => ({ id: track.id, kind: track.kind, state: track.readyState })),
      videos: [...document.querySelectorAll<HTMLVideoElement>('.call-grid video')].flatMap(video =>
        video.srcObject instanceof MediaStream ? video.srcObject.getVideoTracks().map(track => ({ track: track.id,
          width: video.videoWidth, height: video.videoHeight, frames: video.getVideoPlaybackQuality().totalVideoFrames, paused: video.paused })) : []) };
  });
}
async function record(action: string, who: Participant) {
  const value = await snapshot(who.page); observations.push({ action, observer: who.pseudonym, value }); return value;
}
async function relayed(who: Participant) {
  await expect.poll(async () => {
    const current = await snapshot(who.page);
    const connected = current.peers.map((peer, index) => peer.connection === 'connected' ? index : -1).filter(index => index >= 0);
    return connected.length > 0 && connected.every(peer => current.pairs.some(pair => pair.peer === peer
      && pair.state === 'succeeded' && (pair.local === 'relay' || pair.remote === 'relay') && pair.bytesSent > 0 && pair.bytesReceived > 0));
  }, { timeout: 90_000, message: 'Every connected Annex media transport must select an actual relay candidate and carry bytes' }).toBe(true);
  return record('selected actual relay candidate', who);
}
async function advancing(receiver: Participant, sender: Participant, source: 'audio' | 'video' | 'screen') {
  const track = `${source}-${sender.pseudonym}`;
  const before = await record(`before ${source}`, receiver);
  await expect.poll(async () => {
    const current = await snapshot(receiver.page);
    return current.incoming.filter(row => row.track === track).some(row => {
      const previous = before.incoming.find(item => item.peer === row.peer && item.id === row.id);
      const packets = row.packets - (previous?.packets ?? 0), bytes = row.bytes - (previous?.bytes ?? 0);
      if (source === 'audio') return packets > 5 && bytes > 100 && row.samples - (previous?.samples ?? 0) > 1000
        && row.energy - (previous?.energy ?? 0) > 0.00001 && row.codec.toLowerCase() === 'audio/opus';
      return packets > 5 && bytes > 500 && row.frames - (previous?.frames ?? 0) > 5
        && current.videos.some(video => video.track === track && video.width > 0 && video.height > 0 && !video.paused
          && video.frames > (before.videos.find(item => item.track === track)?.frames ?? 0) + 3);
    });
  }, { timeout: 60_000, message: `Independent receiver must decode new ${source} from the named participant` }).toBe(true);
  await record(`advancing ${source}`, receiver); await relayed(receiver);
}
async function join(who: Participant, channel: string) {
  await select(who, channel); await who.page.locator('.voice-join-btn').click();
  await expect(who.page.locator('.voice-connected-header')).toContainText('Voice Connected', { timeout: 90_000 });
  await relayed(who);
}
async function ended(who: Participant) {
  await expect.poll(async () => {
    const value = await snapshot(who.page);
    return value.captures.every(track => track.state === 'ended') && value.peers.every(peer => peer.signaling === 'closed');
  }, { timeout: 90_000, message: 'Failed or ended transport must close actual capture and peer connection' }).toBe(true);
  await record('capture and peer connection ended', who);
}
test.afterEach(async () => {
  for (const who of actors) if (!who.page.isClosed()) await record('final observation', who).catch(() => {});
  await test.info().attach('actual-turn-media', { contentType: 'application/json', body: Buffer.from(JSON.stringify({
    controls, observations, boundary: 'Controlled Chromium speech and camera plus actual captured tab, separate native SFU network namespace, real authenticated coturn, actual selected ICE candidate and independent media decoding. Not public WAN or physical hardware acceptance.',
  })) });
  for (const who of actors) await who.context.close();
});

test('actual TURN authentication failure, relayed speech/camera/screen and relay outage recover through ordinary controls', async ({ browser }) => {
  expect(baseURL, 'Set ANNEX_LIVE_URL explicitly for the isolated TURN endpoint').toBe('http://127.0.0.1:3065');
  const status = control('status'); expect(status.ip_forward).toBe(0);
  control('restart-bad-credentials');
  const owner = await participant(browser); const peer = await participant(browser);
  await expect(owner.page.locator('.admin-menu-btn'), 'Fresh real founder is required').toBeVisible();
  const name = `turn-media-${Date.now()}`;
  const created = await owner.context.request.post(`${baseURL}/api/channels`, { headers: owner.headers,
    data: { channel_id: name, name, channel_type: 'Hybrid', federation_scope: 'Local' } });
  expect(created.status()).toBe(200); const channel = await created.json() as Channel;
  expect(channel.channel_id).toBe(name);
  await reload(owner); await reload(peer);

  await test.step('Real rejected relay credentials cannot produce a successful call or leave capture behind', async () => {
    await select(owner, name); await owner.page.locator('.voice-join-btn').click();
    await expect.poll(async () => (await snapshot(owner.page)).peers.length).toBeGreaterThan(0);
    await expect(owner.page.locator('.voice-error[role="alert"]')).toBeVisible({ timeout: 150_000 });
    await ended(owner); await expect(owner.page.locator('.voice-connected-header')).toHaveCount(0);
    const failed = await snapshot(owner.page);
    expect(failed.pairs.some(pair => pair.state === 'succeeded')).toBe(false);
    expect(failed.events.some(event => event.ice === 'failed' || event.connection === 'failed'),
      'The real ICE connection must fail before the app closes it').toBe(true);
  });
  await test.step('Restored real credentials carry attributable speech, camera and screen over selected TURN relays', async () => {
    control('restart'); await reload(owner); await reload(peer);
    await join(owner, name); await join(peer, name);
    await advancing(peer, owner, 'audio'); await advancing(owner, peer, 'audio');
    const screen = await owner.context.newPage();
    await screen.setContent('<!doctype html><title>Annex TURN screen acceptance</title><style>body{background:#173f67;color:white;font:30px sans-serif}i{display:block;width:100px;height:100px;background:#ffc800;animation:move 1s linear infinite alternate}@keyframes move{to{transform:translateX(400px);background:#00df9a}}</style><h1>Actual relayed screen capture</h1><i></i>');
    await owner.page.bringToFront();
    await owner.page.locator('.media-controls').getByTitle('Turn on camera', { exact: true }).click();
    await advancing(peer, owner, 'video');
    await owner.page.locator('.media-controls').getByTitle('Share screen', { exact: true }).click();
    await advancing(peer, owner, 'screen');
  });
  await test.step('The pinned Linux recognizer transcribes the relayed participant audio with correct attribution', async () => {
    const readiness = await read<{ stt_ready: boolean }>(owner, '/api/voice/config-status');
    expect(readiness.stt_ready).toBe(true);
    for (const [receiver, sender] of [[owner, peer], [peer, owner]]) {
      const captions = receiver.page.locator('.voice-caption').filter({
        has: receiver.page.locator('.voice-caption-speaker', { hasText: sender.pseudonym.slice(0, 14) }),
      }).locator('.voice-caption-text');
      await expect.poll(async () => (await captions.allTextContents()).join(' ').replace(/[^\p{L}\p{N}]+/gu, ' '),
        { timeout: 150_000, message: 'Actual received participant audio must produce attributed visible recognition' })
        .toMatch(/Annex Live Session Speech Verification/i);
    }
  });
  await test.step('Stopping the real relay stops media and capture; restoring it permits an explicit fresh call', async () => {
    control('stop-turn');
    await ended(owner); await ended(peer);
    await expect(owner.page.locator('.voice-connected-header')).toHaveCount(0);
    await expect(peer.page.locator('.voice-connected-header')).toHaveCount(0);
    control('start-turn'); await join(owner, name); await join(peer, name);
    await advancing(peer, owner, 'audio'); await advancing(owner, peer, 'audio');
    await owner.page.locator('.media-controls').getByTitle('Leave call', { exact: true }).click(); await ended(owner);
    await peer.page.locator('.media-controls').getByTitle('Leave call', { exact: true }).click(); await ended(peer);
    const roster = await read<{ participant_ids: string[] }>(owner, `/api/channels/${name}/voice/status`);
    expect(roster.participant_ids).toEqual([]);
  });
  for (const who of actors) expect(who.errors).toEqual([]);
});
