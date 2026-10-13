import { expect, test, type BrowserContext, type Page } from '@playwright/test';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { createInterface } from 'node:readline';
import { fileURLToPath } from 'node:url';
import { completeStartup } from '../helpers';
import { admin, baseURL, newActor, publicIdentities, read, select, send, type Actor } from './operator-helpers';
import type { Message } from '../../src/types';

interface Agent { pseudonym: string; mention: string; voiceProfile: { id: string; backend: string } }
interface RuntimeEvent { type: string; agent?: number; action?: string; ok?: boolean; message?: string;
  response?: string; responseMessageId?: string; promptMessageId?: string; model?: string; eval_count?: number;
  id?: string; backend?: string; pseudonym?: string; channelId?: string; channelName?: string; agents?: Agent[] }
interface Ready extends RuntimeEvent { channelId: string; channelName: string; agents: Agent[] }
interface Audio { at: number; peer: number; report: string; track: string; packets: number; bytes: number;
  samples: number; energy: number; codec: string }
declare global { interface Window { __annexBackendAudio?: { peers: RTCPeerConnection[]; samples: Audio[]; errors: string[]; busy: boolean } } }

async function observe(context: BrowserContext) {
  await context.addInitScript(() => {
    const probe = { peers: [] as RTCPeerConnection[], samples: [] as Audio[], errors: [] as string[], busy: false };
    window.__annexBackendAudio = probe;
    const NativePeer = window.RTCPeerConnection;
    window.RTCPeerConnection = class ObservedPeer extends NativePeer {
      constructor(configuration?: RTCConfiguration) { super(configuration); probe.peers.push(this); }
    };
    setInterval(async () => {
      if (probe.busy) return; probe.busy = true;
      try {
        for (const [peer, connection] of probe.peers.entries()) {
          if (connection.signalingState === 'closed') continue;
          let reports: RTCStatsReport;
          try { reports = await connection.getStats(); }
          catch (error) { if (connection.connectionState !== 'closed') probe.errors.push(String(error)); continue; }
          reports.forEach((entry: RTCInboundRtpStreamStats) => {
            if (entry.type !== 'inbound-rtp' || entry.kind !== 'audio') return;
            const receiver = connection.getTransceivers().find(item => item.mid === entry.mid)?.receiver;
            probe.samples.push({ at: Date.now(), peer, report: entry.id,
              track: entry.trackIdentifier || receiver?.track.id || '',
              packets: entry.packetsReceived ?? 0, bytes: entry.bytesReceived ?? 0,
              samples: entry.totalSamplesReceived ?? 0, energy: entry.totalAudioEnergy ?? 0,
              codec: entry.codecId ? reports.get(entry.codecId)?.mimeType ?? '' : '' });
          });
        }
        if (probe.samples.length > 18_000) probe.samples.splice(0,probe.samples.length - 18_000);
      } finally { probe.busy = false; }
    }, 150);
  });
}
async function samples(page: Page, pseudonym: string, since = 0) {
  return page.evaluate(({ pseudonym, since }) => (window.__annexBackendAudio?.samples ?? [])
    .filter(row => row.track === `audio-${pseudonym}` && row.at >= since), { pseudonym, since });
}
function flow(rows: Audio[]) {
  return rows.some((first,index) => rows.slice(index + 1).some(last => last.peer === first.peer && last.report === first.report
    && last.packets > first.packets + 5 && last.bytes > first.bytes + 100
    && last.samples > first.samples + 1000 && last.energy > first.energy + 0.00001 && last.codec.toLowerCase() === 'audio/opus'));
}
async function decoded(page: Page, agent: Agent, since: number) {
  await expect.poll(async () => flow(await samples(page,agent.pseudonym,since)), { timeout: agent.voiceProfile.backend === 'bark' ? 210_000 : 90_000,
    intervals: [250,500,1000], message: `${agent.voiceProfile.backend} must produce increasing decoded audio samples and energy on its own real Opus track` }).toBe(true);
  await expect.poll(() => page.evaluate(id => [...document.querySelectorAll<HTMLAudioElement>('audio[data-webrtc-remote]')]
    .some(element => element.srcObject instanceof MediaStream && element.srcObject.getAudioTracks().some(track => track.id === `audio-${id}`)
      && !element.paused && !element.muted && element.volume > 0),agent.pseudonym)).toBe(true);
  return samples(page,agent.pseudonym,since);
}
async function stopped(page: Page, pseudonym: string) {
  const latest = async () => [...new Map((await samples(page,pseudonym)).map(row => [`${row.peer}:${row.report}`,row])).values()];
  await expect.poll(async () => {
    const before = await latest(); await page.waitForTimeout(2000); const after = await latest();
    return after.every(row => before.some(previous => previous.peer === row.peer && previous.report === row.report
      && previous.packets === row.packets && previous.energy === row.energy && previous.samples === row.samples));
  }, { timeout: 30_000, intervals: [500] }).toBe(true);
  return latest();
}

async function priorUtteranceFinished(page: Page, pseudonym: string) {
  const latest = async () => [...new Map((await samples(page,pseudonym)).map(row => [`${row.peer}:${row.report}`,row])).values()];
  await expect.poll(async () => {
    const before=await latest(); await page.waitForTimeout(2000); const after=await latest();
    // A live idle decoder may continue producing concealed/silent samples.
    // Only actual new received RTP bytes indicate an unfinished publication.
    return after.every(row => before.some(previous => previous.peer===row.peer && previous.report===row.report
      && previous.packets===row.packets && previous.bytes===row.bytes));
  }, {timeout:90_000,intervals:[250]}).toBe(true);
  return latest();
}

test('genuine model responses use their independently verified Bark and system profiles and revocation stops only the targeted voice', async ({ browser }) => {
  const profiles = (process.env.ANNEX_AGENT_VOICE_PROFILES ?? 'bark-live,system-live').split(',');
  expect(profiles).toHaveLength(2); expect(profiles[0]).not.toBe(profiles[1]);
  const model = process.env.ANNEX_AGENT_MODEL ?? 'gpt-oss:20b';
  const events: RuntimeEvent[] = []; const evidence: Record<string, unknown> = {};
  const allowedRefusals = new Set<RuntimeEvent>();
  let runtime: ChildProcessWithoutNullStreams | undefined; let exit: number | null | undefined;
  let human: Actor | undefined; let ready: Ready | undefined;
  try {
    runtime = spawn(process.execPath, [fileURLToPath(new URL('../../../scripts/live/agent.mjs',import.meta.url)),
      '--url',baseURL,'--model-url',process.env.ANNEX_AGENT_MODEL_URL ?? 'http://127.0.0.1:11434',
      '--model',model,'--voice-profiles',profiles.join(',')], {
      cwd:fileURLToPath(new URL('../../../',import.meta.url)), windowsHide:true, stdio:'pipe',
    });
    runtime.on('exit',code => { exit = code; }); runtime.stderr.resume();
    createInterface({ input:runtime.stdout }).on('line',line => {
      try { const event = JSON.parse(line) as RuntimeEvent;
        if (typeof event.type === 'string') { events.push(event); if (event.type === 'ready') ready = event as Ready; }
      } catch { /* Unstructured dependency diagnostics are not acceptance receipts. */ }
    });
    await expect.poll(() => {
      if (exit !== undefined) throw new Error(`Real backend runtime exited before readiness (${exit})`);
      return Boolean(ready);
    }, { timeout:300_000,intervals:[500,1000] }).toBe(true);
    expect(ready!.agents).toHaveLength(2);
    expect(ready!.agents.map(agent => agent.voiceProfile)).toEqual([
      { id:profiles[0],backend:'bark' },{ id:profiles[1],backend:'system' },
    ]);
    expect(ready!.agents[0].pseudonym).not.toBe(ready!.agents[1].pseudonym);

    human = await newActor(browser,false); await observe(human.context);
    await human.context.grantPermissions(['microphone']); await completeStartup(human.page);
    await expect(human.page.locator('.admin-menu-btn')).toHaveCount(0);
    await test.step('The real operator voice selector recovers failed reads and writes, restores default, and persists selected backends', async () => {
      const denied = await human!.context.request.get(`${baseURL}/api/voice-profiles`, { headers: human!.headers });
      expect(denied.status()).toBe(403);
      const identities = await publicIdentities(human!);
      expect(identities).toHaveLength(1); expect(identities[0].pseudonymId).toBeTruthy();
      const boundary = events.length;
      runtime!.stdin.write(`${JSON.stringify({ action: 'grant-operator', pseudonym: identities[0].pseudonymId })}\n`);
      await expect.poll(() => events.slice(boundary).some(event => event.type === 'control'
        && event.action === 'grant-operator' && event.pseudonym === identities[0].pseudonymId && event.ok)).toBe(true);
      await human!.page.reload();
      await expect(human!.page.locator('.admin-menu-btn')).toBeVisible();
      await admin(human!, 'Member Management');
      const catalog = '**/api/voice-profiles';
      await human!.page.route(catalog, route => route.abort('internetdisconnected'));
      await human!.page.getByRole('button', { name: 'Manage agent voices', exact: true }).click();
      const panel = human!.page.getByRole('region', { name: 'Agent voices', exact: true });
      await expect(panel.getByRole('alert')).toContainText('Could not load agent voices');
      await human!.page.unroute(catalog);
      await panel.getByRole('button', { name: 'Refresh agent voices', exact: true }).click();
      const actualCatalog = await read<{ profiles: Array<{ id: string; available: boolean; model: string | null }> }>(human!, '/api/voice-profiles');
      for (const agent of ready!.agents) expect(actualCatalog.profiles).toContainEqual(expect.objectContaining({
        id: agent.voiceProfile.id, model: agent.voiceProfile.backend, available: true,
      }));
      await panel.getByLabel('Agent', { exact: true }).selectOption(ready!.agents[0].pseudonym);
      const voice = panel.getByLabel('Voice profile', { exact: true });
      await expect(voice).toHaveValue(profiles[0]);
      await voice.selectOption('default');
      const assignment = `**/api/agents/${ready!.agents[0].pseudonym}/voice-profile`;
      await human!.page.route(assignment, route => route.abort('internetdisconnected'));
      await panel.getByRole('button', { name: 'Save voice', exact: true }).click();
      await expect(panel.getByRole('alert')).toBeVisible();
      expect(await read(human!, `/api/agents/${ready!.agents[0].pseudonym}`)).toMatchObject({ voice_profile_id: profiles[0] });
      await expect(panel.getByText('Voice assignment saved.', { exact: true })).toHaveCount(0);
      await human!.page.unroute(assignment);
      await panel.getByRole('button', { name: 'Save voice', exact: true }).click();
      await expect(panel.getByText('Voice assignment saved.', { exact: true })).toBeVisible();
      expect(await read(human!, `/api/agents/${ready!.agents[0].pseudonym}`)).toMatchObject({ voice_profile_id: null, voice_backend: 'system' });
      await voice.selectOption(profiles[0]);
      await panel.getByRole('button', { name: 'Save voice', exact: true }).click();
      await expect(panel.getByText('Voice assignment saved.', { exact: true })).toBeVisible();
      await human!.page.getByRole('button', { name: 'Hide agent voices', exact: true }).click();
      await expect(panel).toHaveCount(0);
      await human!.page.reload(); await admin(human!, 'Member Management');
      await human!.page.getByRole('button', { name: 'Manage agent voices', exact: true }).click();
      for (const agent of ready!.agents) {
        await panel.getByLabel('Agent', { exact: true }).selectOption(agent.pseudonym);
        await expect(panel.getByLabel('Voice profile', { exact: true })).toHaveValue(agent.voiceProfile.id);
      }
      evidence.operatorVoiceControls = { catalog: actualCatalog, persisted: await Promise.all(ready!.agents.map(agent =>
        read(human!, `/api/agents/${agent.pseudonym}`))) };
    });
    await select(human,ready!.channelName); await human.page.locator('.voice-join-btn').click();
    await expect(human.page.locator('.voice-connected-header')).toContainText('Voice Connected');
    const mic = human.page.locator('.media-controls').getByTitle('Mute microphone',{exact:true});
    if (await mic.isVisible()) await mic.click();

    const control = async (action: 'revoke-voice' | 'restore-voice' | 'revoke', index: number) => {
      const from=events.length; runtime!.stdin.write(`${JSON.stringify({action,agent:index})}\n`);
      await expect.poll(() => events.slice(from).some(event => event.type==='control' && event.action===action
        && event.agent===index && event.ok===true), { timeout:90_000 }).toBe(true);
    };
    const prompt = async (index: number,label: string,voice=true) => {
      const agent=ready!.agents[index]; const marker=`backend${Date.now()}${index}`;
      // A Bark utterance can still be playing when the model answers the next
      // prompt. Establish a real quiet receiver boundary first so its tail
      // cannot qualify the next synthesis while that synthesis is still busy.
      if (voice) evidence[`${label}-prior-publication-finished`]=await priorUtteranceFinished(human!.page,agent.pseudonym);
      const started=Date.now();
      const question=await send(human!,ready!.channelName,`${agent.mention} ${marker} What is eleven plus twenty three? Include ${marker} and the number in one short sentence.`);
      let inference: RuntimeEvent | undefined;
      await expect.poll(() => {
        inference=events.find(event => event.type==='inference' && event.agent===index && event.response?.includes(marker));
        return Boolean(inference);
      }, {timeout:210_000,intervals:[500,1000]}).toBe(true);
      expect(inference!.eval_count).toBeGreaterThan(0); expect(inference!.model).toBe(model);
      expect(inference!.response).toMatch(/\b34\b/); expect(inference!.promptMessageId).toBe(question.message_id);
      const history=await read<Message[]>(human!,`/api/channels/${ready!.channelId}/messages?limit=100`);
      expect(history.find(message => message.message_id===inference!.responseMessageId)).toMatchObject({
        sender_pseudonym:agent.pseudonym,reply_to_message_id:question.message_id,content:inference!.response,
      });
      const bubble=human!.page.locator('.message').filter({has:human!.page.locator(`.sender[title="${agent.pseudonym}"]`)}).filter({hasText:marker});
      await expect(bubble).toHaveCount(1); await expect(bubble.locator('.reply-context')).toContainText(marker);
      const profile=await read<{voice_profile_id:string;voice_backend:string}>(human!,`/api/agents/${agent.pseudonym}`);
      expect(profile).toMatchObject({voice_profile_id:profiles[index],voice_backend:agent.voiceProfile.backend});
      evidence[label]={agent:agent.pseudonym,profile:agent.voiceProfile,inference,receiver:voice?await decoded(human!.page,agent,started):[]};
    };

    await test.step('Fresh actual model text is synthesized by Bark and decoded on its attributed native track', async () => { await prompt(0,'bark'); });
    await test.step('A distinct actual model identity uses the assigned system engine and its own native track', async () => {
      await prompt(1,'system');
      const tracks=await human!.page.evaluate(() => [...new Set((window.__annexBackendAudio?.samples ?? []).filter(row => row.energy>0 && row.samples>0).map(row => row.track))]);
      for (const agent of ready!.agents) expect(tracks).toContain(`audio-${agent.pseudonym}`);
    });
    await test.step('Removing Bark voice capability during actual speech stops that publisher while system speech remains usable', async () => {
      await prompt(0,'bark-before-revocation');
      await expect.poll(async () => flow(await samples(human!.page,ready!.agents[0].pseudonym,Date.now()-1200)), {
        timeout:5000,intervals:[100],message:'Revocation must interrupt actual flowing Bark audio',
      }).toBe(true);
      const boundary=events.length; await control('revoke-voice',0);
      evidence.barkStopped=await stopped(human!.page,ready!.agents[0].pseudonym);
      await expect.poll(async () => (await read<{participant_ids:string[]}>(human!,`/api/channels/${ready!.channelId}/voice/status`)).participant_ids.includes(ready!.agents[0].pseudonym)).toBe(false);
      const denial=events.length; await prompt(0,'bark-text-retained-after-voice-revoke',false);
      await expect.poll(() => events.slice(denial).some(event => event.type==='error' && event.agent===0
        && event.message==='voice is not enabled for this identity')).toBe(true);
      await stopped(human!.page,ready!.agents[0].pseudonym);
      for (const event of events.slice(boundary)) if (event.type==='error' && event.agent===0 && [
        'voice is not enabled for this identity','Failed to publish audio: Room service error: Agent voice session has ended',
      ].includes(event.message ?? '')) allowedRefusals.add(event);
      await prompt(1,'system-after-independent-bark-revocation');
    });
    await human.page.getByTitle('Leave call',{exact:true}).click();
    await expect(human.page.locator('audio[data-webrtc-remote]')).toHaveCount(0);
    await expect.poll(() => human!.page.evaluate(() => window.__annexBackendAudio?.peers.every(peer => peer.signalingState==='closed'))).toBe(true);
    expect(human.errors).toEqual([]); expect(await human.page.evaluate(() => window.__annexBackendAudio?.errors)).toEqual([]);
    expect(events.filter(event => event.type==='fatal' || (event.type==='error' && !allowedRefusals.has(event)))).toEqual([]);
  } finally {
    await test.info().attach('actual-bark-system-agent-evidence',{contentType:'application/json',body:Buffer.from(JSON.stringify({
      boundary:'Actual Ollama model, real Annex membership/VRP/WS, operator-assigned loaded Bark and system TTS, native SFU and independent browser decoder. Bark CPU synthesis is an optional high-latency backend, not a fast-voice claim. No Linux Whisper or physical microphone claim.',
      ready,runtime:events,expectedRefusals:[...allowedRefusals],evidence,
    }))});
    await human?.context.close(); if (runtime?.exitCode===null) runtime.kill('SIGTERM');
  }
});
