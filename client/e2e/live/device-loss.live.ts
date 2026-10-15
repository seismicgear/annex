import { chromium, expect, test, type BrowserContext, type Page } from '@playwright/test';
import { createHash } from 'node:crypto';
import { existsSync } from 'node:fs';
import { mkdir, mkdtemp, rm } from 'node:fs/promises';
import { basename, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { completeStartup } from '../helpers';
import { baseURL, newActor, select, type Actor } from './operator-helpers';

/**
 * Real physical selection/capture -> clean browser shutdown -> same private
 * profile with Chromium's controlled device factory. The saved physical IDs
 * must genuinely disappear and native exact-ID capture must reject them.
 * This proves a controlled browser availability transition, not OS unplugging.
 * No capture results, device events, app stores, or API responses are replaced.
 */
interface Inbound { kind: string; bytes: number; packets: number; frames: number; samples: number; energy: number }
declare global { interface Window { __annexDeviceLossPeers?: RTCPeerConnection[] } }
async function observe(context: BrowserContext) {
  await context.addInitScript(() => {
    const peers: RTCPeerConnection[] = []; window.__annexDeviceLossPeers = peers;
    const NativePeer = window.RTCPeerConnection;
    window.RTCPeerConnection = class ObservedPeer extends NativePeer {
      constructor(configuration?: RTCConfiguration) { super(configuration); peers.push(this); }
    };
  });
}
async function inbound(page: Page): Promise<Inbound[]> {
  return page.evaluate(async () => {
    const result: Inbound[] = [];
    for (const connection of window.__annexDeviceLossPeers ?? []) {
      if (connection.connectionState !== 'connected') continue;
      const reports = await connection.getStats();
      reports.forEach(report => {
        if (report.type === 'inbound-rtp') result.push({ kind: report.kind ?? report.mediaType,
          bytes: report.bytesReceived ?? 0, packets: report.packetsReceived ?? 0,
          frames: report.framesDecoded ?? 0, samples: report.totalSamplesReceived ?? 0,
          energy: report.totalAudioEnergy ?? 0 });
      });
    }
    return result;
  });
}
async function actorIn(context: BrowserContext): Promise<Actor> {
  const page = await context.newPage();
  const actor: Actor = { context, page, headers: {}, errors: [] };
  page.on('pageerror', error => actor.errors.push(error.message));
  page.on('request', request => {
    if (new URL(request.url()).origin !== new URL(baseURL).origin) return;
    const headers = request.headers();
    if (headers.authorization && headers['x-annex-zk-proof']) actor.headers = Object.fromEntries(
      Object.entries(headers).filter(([key]) => key === 'authorization' || key.startsWith('x-annex-')));
  });
  await completeStartup(page); return actor;
}
async function settings(page: Page) {
  await page.getByTitle('Audio & Video Settings — change your microphone, speakers, camera, and volume levels', { exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Audio & Video Settings', exact: true });
  await expect(dialog).toBeVisible(); return dialog;
}
async function join(actor: Actor, channel: string) {
  await select(actor, channel); await actor.page.locator('.voice-join-btn').click();
  await expect(actor.page.locator('.voice-connected-header')).toContainText('Voice Connected');
}
const fingerprint = (text: string) => createHash('sha256').update(text).digest('hex');
async function removePrivateProfile(root: string, profile: string) {
  if (!resolve(profile).startsWith(`${resolve(root)}${sep}`) || !basename(profile).startsWith('annex-device-loss-')) {
    throw new Error('Refusing unexpected profile cleanup path');
  }
  await rm(profile, { recursive: true, force: true });
}

test('persisted real camera and microphone selections recover when those devices are absent from the next native browser session', async ({ browser }) => {
  const root = fileURLToPath(new URL('../../../../device-loss-profiles/', import.meta.url));
  await mkdir(root, { recursive: true });
  const profile = await mkdtemp(resolve(root, 'annex-device-loss-'));
  const waveform = process.env.ANNEX_MEDIA_AUDIO_WAV
    ? resolve(process.env.ANNEX_MEDIA_AUDIO_WAV) : fileURLToPath(new URL('../../../../annex-tts.wav', import.meta.url));
  expect(existsSync(waveform), 'A real controlled speech waveform is required for received-audio proof').toBe(true);
  const common = ['--disable-gpu', '--auto-accept-camera-and-microphone-capture', '--autoplay-policy=no-user-gesture-required'];
  let ownerContext: BrowserContext | undefined; let peer: Actor | undefined;
  const receipts: Record<string, unknown> = {};
  try {
    ownerContext = await chromium.launchPersistentContext(profile, { baseURL, headless: false,
      permissions: ['microphone', 'camera'], args: common });
    let owner = await actorIn(ownerContext);
    await expect(owner.page.locator('.admin-menu-btn'), 'A fresh dedicated server is required').toBeVisible();
    peer = await newActor(browser, false); await observe(peer.context); await completeStartup(peer.page);
    const channel = `device-loss-${Date.now()}`;
    const created = await owner.context.request.post(`${baseURL}/api/channels`, { headers: owner.headers,
      data: { channel_id: channel, name: channel, channel_type: 'Hybrid', federation_scope: 'Local' } });
    expect(created.status()).toBe(200);
    await owner.page.reload(); await peer.page.reload();
    await expect(owner.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible();
    await expect(peer.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible();

    let saved: { inputDeviceId: string; cameraDeviceId: string };
    await test.step('Select and capture actual physical device IDs through the real settings controls', async () => {
      const devices = await owner.page.evaluate(async () => {
        const permission = await navigator.mediaDevices.getUserMedia({ audio: true, video: true });
        permission.getTracks().forEach(track => track.stop());
        return (await navigator.mediaDevices.enumerateDevices()).map(device => ({
          id: device.deviceId, kind: device.kind, label: device.label }));
      });
      const real = (kind: string) => devices.find(device => device.kind === kind && device.id
        && device.id !== 'default' && device.id !== 'communications' && device.label
        && !/fake|synthetic|controlled/i.test(device.label));
      const microphone = real('audioinput'); const camera = real('videoinput');
      expect(microphone, 'An actual physical microphone must exist before testing its saved ID becoming unavailable').toBeDefined();
      expect(camera, 'An actual physical camera must exist before testing its saved ID becoming unavailable').toBeDefined();
      saved = { inputDeviceId: microphone!.id, cameraDeviceId: camera!.id };
      const dialog = await settings(owner.page);
      await dialog.getByLabel(/^Input Device/).selectOption(saved.inputDeviceId);
      await dialog.getByLabel(/^Camera/).selectOption(saved.cameraDeviceId);
      await dialog.getByRole('button', { name: 'Done', exact: true }).click();
      expect(await owner.page.evaluate(() => JSON.parse(localStorage.getItem('annex:audioSettings') ?? '{}'))).toMatchObject(saved);
      const physical = await owner.page.evaluate(async devices => {
        const stream = await navigator.mediaDevices.getUserMedia({ audio: { deviceId: { exact: devices.inputDeviceId } },
          video: { deviceId: { exact: devices.cameraDeviceId } } });
        const result = stream.getTracks().map(track => ({ kind: track.kind, state: track.readyState, device: track.getSettings().deviceId }));
        stream.getTracks().forEach(track => track.stop()); return result;
      }, saved);
      expect(physical).toEqual(expect.arrayContaining([
        { kind: 'audio', state: 'live', device: saved.inputDeviceId },
        { kind: 'video', state: 'live', device: saved.cameraDeviceId },
      ]));
      receipts.physicalSelection = { cameraIdSha256: fingerprint(saved.cameraDeviceId), microphoneIdSha256: fingerprint(saved.inputDeviceId), actualCapture: true };
    });

    await test.step('Restart the same profile with a genuinely different browser-owned device collection', async () => {
      await ownerContext!.close(); ownerContext = undefined;
      ownerContext = await chromium.launchPersistentContext(profile, { baseURL, headless: false,
        permissions: ['microphone', 'camera'], args: [...common, '--use-fake-device-for-media-stream',
          `--use-file-for-fake-audio-capture=${waveform}`] });
      owner = await actorIn(ownerContext);
      expect(await owner.page.evaluate(() => JSON.parse(localStorage.getItem('annex:audioSettings') ?? '{}'))).toMatchObject(saved!);
      const absent = await owner.page.evaluate(async selected => {
        const devices = await navigator.mediaDevices.enumerateDevices();
        const present = devices.map(device => device.deviceId);
        const probe = async (kind: 'audio' | 'video', id: string) => {
          try {
            const stream = await navigator.mediaDevices.getUserMedia({ [kind]: { deviceId: { exact: id } } });
            stream.getTracks().forEach(track => track.stop()); return 'unexpected-success';
          } catch (error) { return error instanceof DOMException ? error.name : 'unexpected-error'; }
        };
        return { cameraAbsent: !present.includes(selected.cameraDeviceId), micAbsent: !present.includes(selected.inputDeviceId),
          cameraRefusal: await probe('video', selected.cameraDeviceId), micRefusal: await probe('audio', selected.inputDeviceId),
          permissions: await Promise.all(['microphone', 'camera'].map(async name => (await navigator.permissions.query({ name: name as PermissionName })).state)),
          audioAvailable: devices.some(device => device.kind === 'audioinput'), videoAvailable: devices.some(device => device.kind === 'videoinput') };
      }, saved!);
      expect(absent).toMatchObject({ cameraAbsent: true, micAbsent: true, audioAvailable: true, videoAvailable: true, permissions: ['granted', 'granted'] });
      expect(['NotFoundError', 'OverconstrainedError']).toContain(absent.cameraRefusal);
      expect(['NotFoundError', 'OverconstrainedError']).toContain(absent.micRefusal);
      receipts.transition = absent;
    });

    await test.step('Explicit microphone fallback publishes actual controlled audio to the independent peer', async () => {
      await join(peer!, channel); await join(owner, channel);
      const mic = owner.page.locator('.media-controls').getByTitle('Unmute microphone', { exact: true });
      await expect(mic).toBeVisible(); await mic.click();
      await expect(owner.page.locator('.media-controls .media-error')).toContainText('Saved microphone not found or disconnected — using the default device instead.');
      await expect(owner.page.locator('.media-controls').getByTitle('Mute microphone', { exact: true })).toBeVisible();
      await expect.poll(async () => (await inbound(peer!.page)).some(row => row.kind === 'audio' && row.packets > 10 && row.samples > 1000 && row.energy > 0), { timeout: 60_000 }).toBe(true);
      receipts.receivedAudio = (await inbound(peer!.page)).filter(row => row.kind === 'audio');
      await owner.page.locator('.media-controls .media-error').getByRole('button', { name: 'Dismiss', exact: true }).click();
    });

    await test.step('Missing saved camera requires the real Use default camera action before peer decoding resumes', async () => {
      await owner.page.locator('.media-controls').getByTitle('Turn on camera', { exact: true }).click();
      const warning = owner.page.locator('.stale-camera-recovery');
      await expect(warning).toContainText('Saved camera not found or disconnected.');
      expect((await inbound(peer!.page)).filter(row => row.kind === 'video' && row.frames > 0)).toEqual([]);
      await warning.getByRole('button', { name: 'Use default camera', exact: true }).click();
      await expect(warning).toBeHidden();
      await expect(owner.page.locator('.media-controls').getByTitle('Turn off camera', { exact: true })).toBeVisible();
      await expect.poll(async () => (await inbound(peer!.page)).some(row => row.kind === 'video' && row.frames > 15 && row.bytes > 1000), { timeout: 60_000 }).toBe(true);
      expect(await owner.page.evaluate(() => JSON.parse(localStorage.getItem('annex:audioSettings') ?? '{}').cameraDeviceId)).toBeNull();
      receipts.receivedVideo = (await inbound(peer!.page)).filter(row => row.kind === 'video');
      await owner.page.getByTitle('Leave call', { exact: true }).click();
      await peer!.page.getByTitle('Leave call', { exact: true }).click();
      await expect(owner.page.getByRole('region', { name: 'Voice call', exact: true })).toBeHidden();
      await expect(peer!.page.getByRole('region', { name: 'Voice call', exact: true })).toBeHidden();
    });
    expect(owner.errors).toEqual([]); expect(peer.errors).toEqual([]);
    await test.info().attach('actual-device-availability-transition', { contentType: 'application/json',
      body: Buffer.from(JSON.stringify({ source: 'physical selection followed by controlled Chromium device factory', physicalUnplugProven: false, ...receipts })) });
  } finally {
    await ownerContext?.close(); await peer?.context.close();
    // The disposable profile contains private identity material. It is never
    // attached to evidence or retained in the repository/output artifacts.
    await removePrivateProfile(root, profile);
  }
});
