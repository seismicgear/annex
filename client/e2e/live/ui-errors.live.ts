import { expect, test, type CDPSession } from '@playwright/test';
import type { Channel, ServerPolicy } from '../../src/types';
import type { MemberInfo, OutboxPage } from '../../src/api/admin';
import type { VoiceConfigStatus, VoiceStatus } from '../../src/api/voice';
import { admin, baseURL, mutation, newActor, publicIdentities, read, reload, savePolicy, select, type Actor } from './operator-helpers';

// Request blocking is a real browser transport failure, never a fulfilled or
// fabricated response. CDP permission changes use Chromium's permission store.
const actors: Actor[] = [];
const permissionSessions = new Map<Actor, CDPSession>();
declare global { interface Window { __annexPermissionTracks?: MediaStreamTrack[] } }
async function observeCapture(actor: Actor) {
  // Observe successful native captures without substituting any media API
  // result, stream, permission decision or device. Reload reinstalls this.
  const install = () => {
    const captured: MediaStreamTrack[] = [];
    window.__annexPermissionTracks = captured;
    const native = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
    navigator.mediaDevices.getUserMedia = async constraints => {
      const stream = await native(constraints);
      captured.push(...stream.getTracks());
      return stream;
    };
  };
  await actor.context.addInitScript(install);
  await actor.page.evaluate(install);
}
const run = `errors-${Date.now()}`;
let founder: Actor;
let member: Actor;
let voice: Channel;
let policy: ServerPolicy;

async function network(actor: Actor, paths: string[]): Promise<CDPSession> {
  const session = await actor.context.newCDPSession(actor.page);
  await session.send('Network.enable');
  await session.send('Network.setBlockedURLs', { urls: paths.map(path => `${baseURL}${path}`) });
  return session;
}
async function restore(session: CDPSession) {
  await session.send('Network.setBlockedURLs', { urls: [] });
  await session.detach();
}
async function capabilities(operator: Actor, target: string, changes: Partial<MemberInfo>) {
  const before = (await read<{ members: MemberInfo[] }>(operator, '/api/admin/members')).members.find(item => item.pseudonym_id === target)!;
  expect(before).toBeDefined();
  const next = { ...before, ...changes };
  const response = await operator.context.request.patch(`${baseURL}/api/admin/members/${target}/capabilities`, {
    headers: operator.headers,
    data: { can_voice: next.can_voice, can_moderate: next.can_moderate, can_invite: next.can_invite,
      can_federate: next.can_federate, can_bridge: next.can_bridge },
  });
  expect(response.status()).toBe(200);
  return before;
}
async function createVoice(actor: Actor): Promise<Channel> {
  await actor.page.getByRole('button', { name: 'Chat', exact: true }).click();
  await actor.page.locator('.create-channel-btn').click();
  const dialog = actor.page.getByRole('dialog');
  await dialog.getByPlaceholder('general').fill(`${run}-voice`);
  await dialog.locator('select').selectOption('Hybrid');
  await mutation(actor, 'POST', '/api/channels', () => dialog.getByRole('button', { name: 'Create', exact: true }).click());
  await expect(dialog).toBeHidden();
  return (await read<Channel[]>(actor, '/api/channels')).find(item => item.name === `${run}-voice`)!;
}
async function permission(actor: Actor, setting: 'denied' | 'granted') {
  let session = permissionSessions.get(actor);
  if (!session) {
    session = await actor.context.newCDPSession(actor.page);
    permissionSessions.set(actor, session);
  }
  const { targetInfo } = await session.send('Target.getTargetInfo');
  for (const name of ['microphone', 'camera']) {
    await session.send('Browser.setPermission', {
      permission: { name }, setting, origin: new URL(baseURL).origin,
      browserContextId: targetInfo.browserContextId,
    });
  }
  // Chromium removes this session's permission override when it detaches.
  // Keep it attached throughout actual capture and release at context cleanup.
  expect(await actor.page.evaluate(async () => Promise.all(['microphone', 'camera'].map(async name =>
    (await navigator.permissions.query({ name: name as PermissionName })).state)))).toEqual([setting, setting]);
}
async function join(actor: Actor) {
  await select(actor, voice.name);
  await actor.page.locator('.voice-join-btn').click();
  await expect(actor.page.locator('.voice-connected-header')).toContainText('Voice Connected');
}
async function leave(actor: Actor) {
  await actor.page.getByTitle('Leave call', { exact: true }).click();
  await expect(actor.page.getByRole('region', { name: 'Voice call', exact: true })).toBeHidden();
}

test.describe('Actual browser permission and operator error recovery', () => {
  test.describe.configure({ mode: 'serial' });
  test.beforeAll(async ({ browser }) => {
    founder = await newActor(browser); actors.push(founder);
    await observeCapture(founder);
    await expect(founder.page.locator('.admin-menu-btn'), 'Dedicated fresh database required').toBeVisible();
    member = await newActor(browser); actors.push(member);
    await observeCapture(member);
    policy = await read<ServerPolicy>(founder, '/api/admin/policy');
    voice = await createVoice(founder);
    await select(founder, voice.name);
    await reload(member); await select(member, voice.name);
  });
  test.afterAll(async () => { for (const actor of actors) await actor.context.close(); });
  test.afterEach(() => { for (const actor of actors) expect(actor.errors, 'Uncaught browser exceptions').toEqual([]); });

  test('create permission revoked while dialog is open refuses persistence and the same form recovers', async () => {
    const founderId = (await publicIdentities(founder))[0].pseudonymId!;
    const memberId = (await publicIdentities(member))[0].pseudonymId!;
    const originalMember = await capabilities(founder, memberId, { can_moderate: true });
    await founder.page.locator('.create-channel-btn').click();
    const dialog = founder.page.getByRole('dialog');
    const name = `${run}-refused-create`;
    await dialog.getByPlaceholder('general').fill(name);
    const originalFounder = await capabilities(member, founderId, { can_moderate: false });
    try {
      const denied = founder.page.waitForResponse(response => response.request().method() === 'POST'
        && new URL(response.url()).pathname === '/api/channels');
      await dialog.getByRole('button', { name: 'Create', exact: true }).click();
      expect((await denied).status()).toBe(403);
      await expect(dialog.getByRole('alert')).toBeVisible();
      await expect(dialog.getByPlaceholder('general')).toHaveValue(name);
      expect((await read<Channel[]>(member, '/api/channels')).filter(item => item.name === name)).toEqual([]);
      await capabilities(member, founderId, originalFounder);
      await mutation(founder, 'POST', '/api/channels', () => dialog.getByRole('button', { name: 'Create', exact: true }).click());
      await expect(dialog).toBeHidden();
      expect((await read<Channel[]>(member, '/api/channels')).filter(item => item.name === name)).toHaveLength(1);
    } finally {
      await capabilities(member, founderId, originalFounder);
      await capabilities(founder, memberId, originalMember);
    }
  });

  test('identity grants and member roster fail visibly then Retry obtains the actual authorized lists', async () => {
    for (const [path, message] of [
      ['/api/profile/username/grants', 'Could not load who can see your username:'],
      ['/api/members', 'Could not load the member list:'],
    ] as const) {
      const blocked = await network(member, [path]);
      try {
        await member.page.getByRole('button', { name: 'Identity', exact: true }).click();
        const dialog = member.page.getByRole('dialog');
        const alert = dialog.getByRole('alert').filter({ hasText: message });
        await expect(alert).toBeVisible();
        await expect(dialog.getByText('No other members on this server yet.', { exact: true })).toBeHidden();
        await blocked.send('Network.setBlockedURLs', { urls: [] });
        await mutation(member, 'GET', path, () => alert.getByRole('button', { name: 'Retry', exact: true }).click());
        await expect(alert).toBeHidden();
        const roster = await read<{ members: Array<{ pseudonym_id: string }> }>(member, '/api/members');
        const own = (await publicIdentities(member))[0].pseudonymId;
        const expected = roster.members.filter(item => item.pseudonym_id !== own);
        await expect(dialog.locator('.member-row')).toHaveCount(expected.length);
        for (const item of expected) await expect(dialog.locator(`.member-pseudonym[title="${item.pseudonym_id}"]`)).toBeVisible();
        const grants = await read<{ grantees: string[] }>(member, '/api/profile/username/grants');
        for (const item of expected) {
          const row = dialog.locator('.member-row').filter({ has: member.page.locator(`[title="${item.pseudonym_id}"]`) });
          await expect(row.getByRole('button', { name: grants.grantees.includes(item.pseudonym_id) ? 'Revoke' : 'Grant', exact: true })).toBeVisible();
        }
        await dialog.getByRole('button', { name: 'Close', exact: true }).click();
      } finally { await restore(blocked); }
    }
  });

  test('member sidebar and federation queue transport failures do not masquerade as empty results', async () => {
    const membersBlocked = await network(member, ['/api/public/agents']);
    try {
      await reload(member);
      const alert = member.page.locator('aside.member-list').getByRole('alert');
      await expect(alert).toContainText('Could not load the member list');
      await membersBlocked.send('Network.setBlockedURLs', { urls: [] });
      await mutation(member, 'GET', '/api/public/agents', () => alert.getByRole('button', { name: 'Retry', exact: true }).click());
      await expect(alert).toBeHidden();
      const summary = await read<{ members_by_type: Record<string, number> }>(member, '/api/public/server/summary');
      for (const [kind, count] of Object.entries(summary.members_by_type)) {
        const label = ({ HUMAN: 'Human', AI_AGENT: 'Agent', COLLECTIVE: 'Collective', BRIDGE: 'Bridge', SERVICE: 'Service' } as Record<string, string>)[kind] ?? kind;
        await expect(member.page.locator('.member-count').filter({ hasText: label }).locator('.count')).toHaveText(String(count));
      }
    } finally { await restore(membersBlocked); }
    const blocked = await network(founder, ['/api/admin/federation/outbox*']);
    try {
      await admin(founder, 'Federation Delivery');
      const panel = founder.page.locator('.federation-outbox');
      await expect(panel.getByRole('alert')).toContainText('Could not read the delivery queue:');
      await expect(panel.locator('.outbox-empty')).toBeHidden();
      await expect(panel.locator('.outbox-counts')).toBeHidden();
      await blocked.send('Network.setBlockedURLs', { urls: [] });
      await mutation(founder, 'GET', '/api/admin/federation/outbox', () => panel.getByRole('button', { name: 'Refresh delivery queue', exact: true }).click());
      await expect(panel.getByRole('alert')).toBeHidden();
      for (const status of ['', 'pending', 'failed', 'paused', 'delivered', 'cancelled']) {
        const response = founder.page.waitForResponse(response => response.request().method() === 'GET'
          && new URL(response.url()).pathname === '/api/admin/federation/outbox');
        if (status === '') await panel.getByRole('button', { name: 'Refresh delivery queue', exact: true }).click();
        else await panel.getByRole('combobox', { name: /^Show/ }).selectOption(status);
        const current = await (await response).json() as OutboxPage;
        await expect(panel.locator('.outbox-row')).toHaveCount(current.entries.length);
        for (const [name, count] of Object.entries(current.counts)) await expect(panel.locator(`.outbox-count-${name} .outbox-count-n`)).toHaveText(String(count));
      }
    } finally { await restore(blocked); }
  });

  test('actual disabled voice policy, permission-read outage and refused join recover through visible controls', async () => {
    await admin(founder, 'Server Policy');
    await founder.page.getByRole('checkbox', { name: /^Voice Enabled/ }).uncheck();
    await savePolicy(founder);
    try {
      expect((await read<VoiceConfigStatus>(member, '/api/voice/config-status')).policy_enabled).toBe(false);
      await reload(member); await select(member, voice.name);
      await expect(member.page.locator('.voice-join-btn')).toBeDisabled();
      await expect(member.page.locator('.voice-error')).toContainText('Voice is turned off for this server.');
    } finally {
      await founder.page.getByRole('checkbox', { name: /^Voice Enabled/ }).setChecked(policy.voice_enabled);
      await savePolicy(founder);
    }
    expect(policy.voice_enabled, 'This lane needs a genuinely configured, enabled voice server').toBe(true);
    const identityId = (await publicIdentities(member))[0].pseudonymId!;
    const blocked = await network(member, [`/api/identity/${identityId}`]);
    try {
      await reload(member); await select(member, voice.name);
      const notice = member.page.locator('.voice-permissions-notice').filter({ hasText: 'Could not verify voice permissions right now.' });
      await expect(notice).toBeVisible();
      await expect(member.page.locator('.voice-join-btn')).toBeDisabled();
      await blocked.send('Network.setBlockedURLs', { urls: [] });
      await mutation(member, 'GET', `/api/identity/${identityId}`, () => notice.getByRole('button', { name: 'Retry', exact: true }).click());
      await expect(notice).toBeHidden();
      await expect(member.page.locator('.voice-join-btn')).toBeEnabled();
      await blocked.send('Network.setBlockedURLs', { urls: [`${baseURL}/api/channels/${voice.channel_id}/voice/join`] });
      const failed = member.page.waitForEvent('requestfailed', { predicate: request => new URL(request.url()).pathname.endsWith('/voice/join') });
      await member.page.locator('.voice-join-btn').click(); await failed;
      await expect(member.page.locator('.voice-error')).toBeVisible();
      await expect(member.page.getByRole('region', { name: 'Voice call', exact: true })).toBeHidden();
      expect((await read<VoiceStatus>(founder, `/api/channels/${voice.channel_id}/voice/status`)).participants).toBe(0);
      await blocked.send('Network.setBlockedURLs', { urls: [] });
      await permission(member, 'denied');
      await join(member);
      expect((await read<VoiceStatus>(founder, `/api/channels/${voice.channel_id}/voice/status`)).participant_ids).toEqual([identityId]);
      await leave(member);
    } finally { await restore(blocked); }
  });

  test('actual browser camera/microphone denial keeps diagnostics off, permits listen-only call and dismisses errors', async () => {
    const info = test.info();
    await permission(founder, 'denied'); await permission(member, 'denied');
    await join(founder); await join(member);
    try {
      const identities = [(await publicIdentities(founder))[0].pseudonymId!, (await publicIdentities(member))[0].pseudonymId!].sort();
      await expect.poll(async () => (await read<VoiceStatus>(founder, `/api/channels/${voice.channel_id}/voice/status`)).participant_ids).toEqual(identities);
      for (const [title, text] of [['Unmute microphone', 'Unmute microphone: permission denied.'], ['Turn on camera', 'Turn on camera: permission denied.']] as const) {
        await member.page.locator('.media-controls').getByTitle(title, { exact: true }).click();
        const error = member.page.locator('.media-controls .media-error');
        await expect(error).toContainText(text);
        await expect(member.page.locator('.local-media-status')).toContainText('Mic OFF');
        await expect(member.page.locator('.local-media-status')).toContainText('Cam OFF');
        await expect(member.page.locator('.local-media-status .on')).toHaveCount(0);
        // Negotiated receiver tracks can exist and be unmuted without any
        // decoded media. Neither is a capture oracle: verify both native
        // capture observers and the recipient's actual decoder instead.
        for (const actor of [founder, member]) {
          expect(await actor.page.evaluate(() => window.__annexPermissionTracks?.length)).toBe(0);
        }
        const videoStates = await member.page.locator('.call-grid video').evaluateAll(elements => elements.map(element => {
          const video = element as HTMLVideoElement;
          return { decodedFrames: video.getVideoPlaybackQuality().totalVideoFrames };
        }));
        for (const state of videoStates) expect(state).toEqual({ decodedFrames: 0 });
        await error.getByRole('button', { name: 'Dismiss', exact: true }).click();
        await expect(error).toBeHidden();
      }
      const config = await read<VoiceConfigStatus>(founder, '/api/voice/config-status');
      if (process.env.ANNEX_EXPECT_STT_UNAVAILABLE === '1') {
        expect(config.stt_ready).toBe(false);
        expect(config.stt_detail?.trim()).toBeTruthy();
        await expect(member.page.getByLabel('Live captions', { exact: true })).toContainText(config.stt_detail!);
        await expect(member.page.getByLabel('Live captions', { exact: true })).toContainText('Live captions are unavailable.');
      }
      await permission(member, 'granted');
      // Physical recovery is an explicit lane: no synthetic getUserMedia or
      // imaginary device IDs are substituted when hardware is absent.
      if (process.env.ANNEX_UI_ERROR_PHYSICAL_RECOVERY === '1') {
        const kinds = await member.page.evaluate(async () => (await navigator.mediaDevices.enumerateDevices()).map(device => device.kind));
        expect(kinds).toContain('audioinput'); expect(kinds).toContain('videoinput');
        await member.page.locator('.media-controls').getByTitle('Unmute microphone', { exact: true }).click();
        await expect(member.page.locator('.local-media-status')).toContainText('Mic ON');
        await member.page.locator('.media-controls').getByTitle('Turn on camera', { exact: true }).click();
        await expect(member.page.locator('.local-media-status')).toContainText('Cam ON');
        await expect.poll(() => member.page.evaluate(() => (window.__annexPermissionTracks ?? [])
          .filter(track => track.readyState === 'live' && track.enabled).map(track => track.kind).sort())).toEqual(['audio', 'video']);
        await expect.poll(() => founder.page.locator('.call-grid video').evaluateAll(elements => elements.some(element => {
          const video = element as HTMLVideoElement;
          return video.videoWidth > 0 && video.getVideoPlaybackQuality().totalVideoFrames > 5;
        }))).toBe(true);
        await member.page.locator('.media-controls').getByTitle('Turn off camera', { exact: true }).click();
        await member.page.locator('.media-controls').getByTitle('Mute microphone', { exact: true }).click();
        await expect.poll(() => member.page.evaluate(() => (window.__annexPermissionTracks ?? [])
          .every(track => track.readyState === 'ended'))).toBe(true);
      }
      await info.attach('actual-browser-permission-evidence', { contentType: 'application/json', body: Buffer.from(JSON.stringify({
        channelId: voice.channel_id, participants: identities, browserPermissions: 'denied then granted',
        mediaSource: 'physical browser APIs only; no fake devices',
        physicalRecoveryRequired: process.env.ANNEX_UI_ERROR_PHYSICAL_RECOVERY === '1',
        missingSttRequired: process.env.ANNEX_EXPECT_STT_UNAVAILABLE === '1', sttReady: config.stt_ready,
      })) });
    } finally { await leave(member); await leave(founder); }
    expect((await read<VoiceStatus>(founder, `/api/channels/${voice.channel_id}/voice/status`)).participants).toBe(0);
  });
});
