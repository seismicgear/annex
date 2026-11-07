import { test, expect, type Page } from '@playwright/test';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { completeStartup } from '../helpers';
import type { Message } from '../../src/types';
import { type Actor, baseURL, newActor, publicIdentities, read, select, send } from './operator-helpers';

/** Actual model, authentication, WS, Piper, native SFU, browser decoder and Whisper. */
interface AgentIdentity { pseudonym: string; mention: string }
interface Ready { type: 'ready'; channelId: string; channelName: string; agents: AgentIdentity[] }
interface RuntimeEvent {
  type: string; at?: string; action?: string; agent?: number; ok?: boolean;
  channelId?: string; speakerPseudonym?: string; text?: string; response?: string;
  responseMessageId?: string; eval_count?: number; model?: string; message?: string;
}
interface AudioSample {
  at: number; peer: number; statId: string; trackId: string; packets: number;
  bytes: number; energy: number; samples: number; codec: string;
}
interface AgentProbe { peers: RTCPeerConnection[]; samples: AudioSample[]; inFlight: boolean; errors: string[] }
declare global { interface Window { __annexAgentAcceptance?: AgentProbe } }
let processHandle: ChildProcessWithoutNullStreams | undefined;
let human: Actor | undefined;
const events: RuntimeEvent[] = [];
let ready: Ready | undefined;
const measurements: Array<{ step: string; samples: AudioSample[] }> = [];
const expectedRuntimeErrors = new Set<RuntimeEvent>();
let processExit: number | null | undefined;

async function observeNativeAudio(who: Actor) {
  await who.context.addInitScript(() => {
    const probe: AgentProbe = { peers: [], samples: [], inFlight: false, errors: [] };
    window.__annexAgentAcceptance = probe;
    const NativePeer = window.RTCPeerConnection;
    window.RTCPeerConnection = class ObservedPeer extends NativePeer {
      constructor(configuration?: RTCConfiguration) { super(configuration); probe.peers.push(this); }
    };
    // Observe native reports while the external model thinks and speaks. This
    // never changes capture, transport, playback or an application outcome.
    setInterval(async () => {
      if (probe.inFlight) return;
      probe.inFlight = true;
      try {
        for (const [index, pc] of probe.peers.entries()) {
          if (pc.signalingState === 'closed') continue;
          let reports: RTCStatsReport;
          try { reports = await pc.getStats(); }
          catch (error) {
            // A connection may close after the check above; do not manufacture
            // an application exception from that legitimate cleanup race.
            if (pc.connectionState !== 'closed') probe.errors.push(String(error));
            continue;
          }
          reports.forEach((entry: RTCInboundRtpStreamStats) => {
            if (entry.type !== 'inbound-rtp' || entry.kind !== 'audio') return;
            const transceiver = pc.getTransceivers().find(item => item.mid === entry.mid);
            const codec = entry.codecId ? reports.get(entry.codecId) as { mimeType?: string } | undefined : undefined;
            probe.samples.push({
              at: Date.now(), peer: index, statId: entry.id,
              trackId: entry.trackIdentifier || transceiver?.receiver.track.id || '',
              packets: entry.packetsReceived ?? 0, bytes: entry.bytesReceived ?? 0,
              energy: entry.totalAudioEnergy ?? 0, samples: entry.totalSamplesReceived ?? 0,
              codec: codec?.mimeType ?? '',
            });
          });
        }
        if (probe.samples.length > 12_000) probe.samples.splice(0, probe.samples.length - 12_000);
      } finally { probe.inFlight = false; }
    }, 200);
  });
}

async function audioSamples(page: Page, pseudonym: string, since: number): Promise<AudioSample[]> {
  return page.evaluate(({ id, since }) => (window.__annexAgentAcceptance?.samples ?? [])
    .filter(item => item.trackId === `audio-${id}` && item.at >= since), { id: pseudonym, since });
}
async function proveAudio(page: Page, agent: AgentIdentity, since: number, step: string) {
  await expect.poll(async () => {
    const samples = await audioSamples(page, agent.pseudonym, since);
    return samples.some((first, index) => samples.slice(index + 1).some(last =>
      last.peer === first.peer && last.statId === first.statId && last.packets > first.packets + 5
      && last.bytes > first.bytes + 100 && last.energy > first.energy + 0.00001
      && last.samples > first.samples && last.codec.toLowerCase() === 'audio/opus'));
  }, { timeout: 60_000, intervals: [250, 500, 1_000], message: `${step}: generated speech must actually decode at the independent human browser` }).toBe(true);
  await expect.poll(() => page.evaluate(id => [...document.querySelectorAll<HTMLAudioElement>('audio[data-webrtc-remote]')]
    .some(element => element.srcObject instanceof MediaStream
      && element.srcObject.getAudioTracks().some(track => track.id === `audio-${id}`)
      && !element.paused && !element.muted && element.volume > 0), agent.pseudonym)).toBe(true);
  measurements.push({ step, samples: await audioSamples(page, agent.pseudonym, since) });
}
async function audioStopped(page: Page, pseudonym: string, step: string) {
  const latest = async () => {
    const samples = await audioSamples(page, pseudonym, 0);
    const byStat = new Map<string, AudioSample>();
    for (const entry of samples) byStat.set(`${entry.peer}:${entry.statId}`, entry);
    return [...byStat.values()];
  };
  await expect.poll(async () => {
    const before = await latest();
    await page.waitForTimeout(2_000);
    const after = await latest();
    return after.every(item => {
      const old = before.find(candidate => candidate.peer === item.peer && candidate.statId === item.statId);
      return old && old.packets === item.packets && old.energy === item.energy;
    });
  }, { timeout: 30_000, intervals: [500], message: step }).toBe(true);
  measurements.push({ step, samples: await latest() });
}
async function currentlySpeaking(page: Page, pseudonym: string) {
  await expect.poll(async () => {
    const current = await audioSamples(page, pseudonym, Date.now() - 1_000);
    const last = current.at(-1);
    return Boolean(last && current.some(first => first.statId === last.statId
      && last.packets > first.packets + 3 && last.energy > first.energy));
  }, { timeout: 5_000, intervals: [100], message: 'Lifecycle action must interrupt actual flowing agent audio' }).toBe(true);
}
async function control(action: 'disconnect' | 'reconnect' | 'revoke' | 'revoke-voice' | 'restore-voice', agent: number) {
  const from = events.length;
  processHandle!.stdin.write(`${JSON.stringify({ action, agent })}\n`);
  await expect.poll(() => events.slice(from).some(event => event.type === 'control'
    && event.action === action && event.agent === agent && event.ok === true), {
    timeout: 90_000, message: `Actual runtime must confirm agent ${agent} ${action}`,
  }).toBe(true);
}
async function promptAgent(index: number, label: string, expectVoice = true) {
  const agent = ready!.agents[index];
  const marker = `proof${Date.now()}${index}`;
  const before = Date.now();
  const prompt = `${agent.mention} ${marker} What is seven plus nineteen? Include ${marker} and the number in one short sentence.`;
  const request = await send(human!, ready!.channelName, prompt);
  let inference: RuntimeEvent | undefined;
  await expect.poll(() => {
    inference = events.find(event => event.type === 'inference' && event.agent === index && event.response?.includes(marker));
    return Boolean(inference);
  }, { timeout: 210_000, intervals: [500, 1_000], message: 'Actual installed model must complete this fresh nonce prompt' }).toBe(true);
  expect(inference!.eval_count).toBeGreaterThan(0);
  expect(inference!.model).toBeTruthy();
  // Real model wording varies; both spellings express the exact answer.
  expect(inference!.response).toMatch(/\b(?:26|twenty[- ]six)\b/i);
  const history = await read<Message[]>(human!, `/api/channels/${ready!.channelId}/messages?limit=100`);
  const answer = history.find(item => item.message_id === inference!.responseMessageId);
  expect(answer?.sender_pseudonym).toBe(agent.pseudonym);
  expect(answer?.reply_to_message_id).toBe(request.message_id);
  expect(answer?.content).toBe(inference!.response);
  const bubble = human!.page.locator('.message').filter({
    has: human!.page.locator(`.sender[title="${agent.pseudonym}"]`),
  }).filter({ hasText: marker });
  await expect(bubble).toBeVisible();
  await expect(bubble.locator('.reply-context')).toContainText(marker);
  if (expectVoice) await proveAudio(human!.page, agent, before, label);
  return marker;
}

test.afterEach(async () => {
  await test.info().attach('real-agent-evidence', { body: JSON.stringify({
    boundary: 'Real installed local model, actual Annex identity/VRP/WS, Piper TTS and browser Opus receiver. Human input is a controlled Piper WAV through Chromium capture; no physical hardware claim.',
    publicIdentity: ready, runtime: events, expectedPolicyRefusals: [...expectedRuntimeErrors], receiver: measurements,
  }, null, 2), contentType: 'application/json' });
  if (human) await human.context.close();
  if (processHandle && processHandle.exitCode === null) processHandle.kill('SIGTERM');
});

test('two actual model agents answer, speak independently, hear human speech, detach, reconnect and revoke', async ({ browser }) => {
  const script = fileURLToPath(new URL('../../../scripts/live/agent.mjs', import.meta.url));
  const repo = fileURLToPath(new URL('../../../', import.meta.url));
  processHandle = spawn(process.execPath, [script, '--url', baseURL,
    '--model-url', process.env.ANNEX_AGENT_MODEL_URL ?? 'http://127.0.0.1:11434',
    '--model', process.env.ANNEX_AGENT_MODEL ?? 'gpt-oss:20b'], { cwd: repo, windowsHide: true, stdio: 'pipe' });
  processHandle.on('exit', code => { processExit = code; });
  // Drain stderr so the child cannot block, but do not attach arbitrary process
  // output that could contain credentials. Structured runtime receipts suffice.
  processHandle.stderr.resume();
  createInterface({ input: processHandle.stdout }).on('line', line => {
    try {
      const event = JSON.parse(line) as RuntimeEvent;
      if (typeof event.type !== 'string') return;
      events.push(event);
      if (event.type === 'ready') ready = event as Ready;
    } catch { /* Dependency diagnostic lines are not proof receipts. */ }
  });
  await expect.poll(() => {
    if (processExit !== undefined) throw new Error(`Real agent runtime exited before readiness (${processExit})`);
    return Boolean(ready);
  }, { timeout: 300_000, intervals: [500, 1_000], message: 'Actual founder plus two VRP-registered agents must become ready' }).toBe(true);
  expect(ready!.agents).toHaveLength(2);
  expect(ready!.agents[0].pseudonym).not.toBe(ready!.agents[1].pseudonym);

  human = await newActor(browser, false);
  await human.context.grantPermissions(['microphone', 'camera']);
  await observeNativeAudio(human);
  await completeStartup(human.page);
  const humanPseudonym = (await publicIdentities(human))[0].pseudonymId;
  await expect(human.page.locator('.admin-menu-btn')).toHaveCount(0);
  await select(human, ready!.channelName);
  await expect(human.page.locator('.voice-join-btn')).toBeEnabled();
  const joined = human.page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === `/api/channels/${ready!.channelId}/voice/join`);
  await human.page.locator('.voice-join-btn').click();
  expect((await joined).ok()).toBe(true);
  await expect(human.page.locator('.media-controls')).toBeVisible();

  await test.step('fresh model generations arrive as authored text and two distinct decoded voices', async () => {
    await promptAgent(0, 'first agent response decoded');
    await promptAgent(1, 'second agent response decoded on its own track');
    const ids = await human!.page.evaluate(() => [...new Set((window.__annexAgentAcceptance?.samples ?? [])
      .filter(item => item.bytes > 0 && item.energy > 0).map(item => item.trackId))]);
    for (const agent of ready!.agents) expect(ids).toContain(`audio-${agent.pseudonym}`);
  });
  await test.step('real human speech reaches both voice participants through Whisper', async () => {
    // VoiceIntent establishes each agent's room participation. Ordinary text
    // subscription alone must never grant access to room captions.
    for (let index = 0; index < ready!.agents.length; index++) {
      await expect.poll(() => events.filter(event => event.type === 'transcription' && event.agent === index
        && event.channelId === ready!.channelId && event.speakerPseudonym === humanPseudonym).map(event => event.text ?? '').join(' ')
        .replace(/[^\p{L}\p{N}]+/gu, ' ').trim(), {
        timeout: 150_000, intervals: [1_000], message: `Agent ${index} must hear the real controlled human waveform`,
      }).toMatch(/annex live session speech verification/i);
    }
  });
  await test.step('moderator voice capability removal interrupts speech and captions while text remains authorized', async () => {
    await promptAgent(0, 'actual speaking stream before voice capability removal');
    await currentlySpeaking(human!.page, ready!.agents[0].pseudonym);
    const revocationStart = events.length;
    await control('revoke-voice', 0);
    await audioStopped(human!.page, ready!.agents[0].pseudonym, 'Capability-revoked agent must stop reaching the browser');
    await expect.poll(async () => (await read<{ participant_ids: string[] }>(human!,
      `/api/channels/${ready!.channelId}/voice/status`)).participant_ids.includes(ready!.agents[0].pseudonym)).toBe(false);

    const freshCaptionStart = events.length;
    await expect.poll(() => events.slice(freshCaptionStart).some(event => event.type === 'transcription'
      && event.agent === 1 && event.speakerPseudonym === humanPseudonym && Boolean(event.text?.trim())), {
      timeout: 90_000, intervals: [1_000], message: 'Unrevoked agent must continue receiving live human speech',
    }).toBe(true);
    expect(events.slice(freshCaptionStart).some(event => event.type === 'transcription' && event.agent === 0)).toBe(false);

    const deniedIntentStart = events.length;
    await promptAgent(0, 'voice-revoked identity remains authorized to answer in text', false);
    await expect.poll(() => events.slice(deniedIntentStart).some(event => event.type === 'error' && event.agent === 0
      && event.message === 'voice is not enabled for this identity'), {
      timeout: 30_000, message: 'A fresh intent on the retained socket must observe the current revoked capability',
    }).toBe(true);
    await audioStopped(human!.page, ready!.agents[0].pseudonym, 'Refused fresh intent must not resurrect revoked media');
    // Preserve and classify only the exact deliberate refusal and active-stream
    // cancellation. Every other runtime error still fails final acceptance.
    for (const event of events.slice(revocationStart)) {
      if (event.type === 'error' && event.agent === 0 && (event.message === 'voice is not enabled for this identity'
        || event.message === 'Failed to publish audio: Room service error: Agent voice session has ended')) {
        expectedRuntimeErrors.add(event);
      }
    }
    await promptAgent(1, 'unrevoked agent voice survives capability change');
    await control('restore-voice', 0);
    await promptAgent(0, 'restored capability permits fresh generated voice on the retained socket');
  });
  await test.step('disconnect removes a speaking agent; reconnect restores genuine authentication and voice', async () => {
    // A fresh answer provides an active native stream before disconnect.
    await promptAgent(0, 'first agent before disconnect');
    await currentlySpeaking(human!.page, ready!.agents[0].pseudonym);
    await control('disconnect', 0);
    await audioStopped(human!.page, ready!.agents[0].pseudonym, 'Disconnected agent must stop reaching the browser');
    await expect.poll(async () => (await read<{ participant_ids: string[] }>(human!,
      `/api/channels/${ready!.channelId}/voice/status`)).participant_ids.includes(ready!.agents[0].pseudonym)).toBe(false);
    await promptAgent(1, 'second agent survives independent disconnect');
    await control('reconnect', 0);
    await promptAgent(0, 'reconnected agent authenticates and speaks again');
  });
  await test.step('revocation rejects old credentials and removes only the revoked agent stream', async () => {
    await promptAgent(0, 'actual speaking stream before revocation');
    await currentlySpeaking(human!.page, ready!.agents[0].pseudonym);
    await control('revoke', 0);
    await audioStopped(human!.page, ready!.agents[0].pseudonym, 'Revoked agent voice must stop');
    await expect.poll(async () => (await read<{ participant_ids: string[] }>(human!,
      `/api/channels/${ready!.channelId}/voice/status`)).participant_ids.includes(ready!.agents[0].pseudonym)).toBe(false);
    await promptAgent(1, 'unrevoked agent still answers and speaks');
    const marker = `revoked${Date.now()}`;
    await send(human!, ready!.channelName, `${ready!.agents[0].mention} ${marker} reply with this marker`);
    await human!.page.waitForTimeout(3_000);
    const history = await read<Message[]>(human!, `/api/channels/${ready!.channelId}/messages?limit=100`);
    expect(history.some(item => item.sender_pseudonym === ready!.agents[0].pseudonym && item.content.includes(marker))).toBe(false);
  });
  await human.page.getByTitle('Leave call', { exact: true }).click();
  await expect(human.page.locator('audio[data-webrtc-remote]')).toHaveCount(0);
  await expect.poll(() => human!.page.evaluate(() => window.__annexAgentAcceptance?.peers.every(pc => pc.signalingState === 'closed'))).toBe(true);
  expect(human.errors).toEqual([]);
  expect(await human.page.evaluate(() => window.__annexAgentAcceptance?.errors)).toEqual([]);
  expect(events.filter(event => event.type === 'fatal' || (event.type === 'error' && !expectedRuntimeErrors.has(event)))).toEqual([]);
  const path = test.info().outputPath('actual-model-agent-conversation.png');
  await human.page.screenshot({ path });
  await test.info().attach('actual-model-agent-conversation', { path, contentType: 'image/png' });
});
