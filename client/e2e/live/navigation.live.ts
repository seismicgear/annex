import { test, expect, type Browser, type BrowserContext, type Page, type TestInfo } from '@playwright/test';
import { completeStartup } from '../helpers';
import { realNetworkLink, type RealNetworkLink } from './resilience-network';
import type { AgentInfo, Channel, FederationPeer, Message, ServerPolicy, ServerSummary } from '../../src/types';

const home = process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000';
const remote = process.env.ANNEX_LIVE_SECOND_URL ?? 'http://127.0.0.1:3002';
const run = `navigation-${Date.now()}`;
type Actor = {
  page: Page; context: BrowserContext; errors: string[];
  credentials: Map<string, Record<string, string>>;
};
type PublicIdentity = { id: string; commitmentHex: string; pseudonymId: string | null; serverSlug: string };
type PublicServer = { id: string; identityId: string; baseUrl: string; label: string; slug: string };
const actors: Actor[] = [];
const links: RealNetworkLink[] = [];
let owner: Actor, remoteOwner: Actor;
let homeSummary: ServerSummary, remoteSummary: ServerSummary;
let homeChannel: Channel, remoteChannel: Channel;

function observe(who: Actor, page: Page) {
  page.on('pageerror', error => who.errors.push(error.message));
  page.on('request', request => {
    const headers = request.headers();
    if (!headers.authorization || !headers['x-annex-zk-proof']) return;
    who.credentials.set(new URL(request.url()).origin, Object.fromEntries(
      Object.entries(headers).filter(([key]) => key === 'authorization' || key.startsWith('x-annex-'))));
  });
}
async function actor(browser: Browser, url = home, register = true) {
  const context = await browser.newContext({ baseURL: url });
  const page = await context.newPage();
  const who: Actor = { page, context, credentials: new Map(), errors: [] };
  observe(who, page); actors.push(who);
  if (register) await completeStartup(page);
  return who;
}
async function ready(who: Actor) {
  // Drive only the visible startup continuation; never invent a store state.
  await expect.poll(async () => {
    if (await who.page.getByRole('button', { name: 'Chat', exact: true }).isVisible()) return true;
    const next = who.page.getByRole('button', { name: 'Continue', exact: true });
    if (await next.isVisible()) await next.click();
    return false;
  }, { timeout: 150_000, intervals: [500, 1000] }).toBe(true);
  await expect(who.page.locator('.ws-indicator.connected')).toBeVisible();
}
async function read<T>(who: Actor, origin: string, route: string, authenticated = true): Promise<T> {
  const key = new URL(origin).origin;
  if (authenticated) await expect.poll(() => who.credentials.has(key)).toBe(true);
  const response = await who.context.request.get(new URL(route, origin).href, {
    headers: authenticated ? who.credentials.get(key) : undefined,
  });
  expect(response.status(), `Actual GET ${route}`).toBe(200);
  return response.json() as Promise<T>;
}
async function identities(page: Page): Promise<PublicIdentity[]> {
  return page.evaluate(async () => new Promise<PublicIdentity[]>((resolve, reject) => {
    const opening = indexedDB.open('annex-identity');
    opening.onerror = () => reject(new Error('Identity database read failed'));
    opening.onsuccess = () => {
      const db = opening.result;
      if (!db.objectStoreNames.contains('identities')) { db.close(); resolve([]); return; }
      const request = db.transaction('identities', 'readonly').objectStore('identities').getAll();
      request.onerror = () => { db.close(); reject(new Error('Identity read failed')); };
      request.onsuccess = () => {
        resolve(request.result.map((item: PublicIdentity) => ({ id: item.id, commitmentHex: item.commitmentHex,
          pseudonymId: item.pseudonymId, serverSlug: item.serverSlug })));
        db.close();
      };
    };
  }));
}
async function servers(page: Page): Promise<PublicServer[]> {
  return page.evaluate(async () => new Promise<PublicServer[]>((resolve, reject) => {
    const opening = indexedDB.open('annex-servers');
    opening.onerror = () => reject(new Error('Server database read failed'));
    opening.onsuccess = () => {
      const db = opening.result;
      if (!db.objectStoreNames.contains('servers')) { db.close(); resolve([]); return; }
      const request = db.transaction('servers', 'readonly').objectStore('servers').getAll();
      request.onerror = () => { db.close(); reject(new Error('Server read failed')); };
      request.onsuccess = () => {
        resolve(request.result.map((item: PublicServer) => ({ id: item.id, identityId: item.identityId,
          baseUrl: item.baseUrl, label: item.label, slug: item.slug })));
        db.close();
      };
    };
  }));
}
function bubble(page: Page, marker: string) {
  return page.locator('.message').filter({ has: page.locator('.message-content').filter({ hasText: marker }) });
}
async function select(who: Actor, name: string) {
  await who.page.getByRole('button', { name: 'Chat', exact: true }).click();
  const row = who.page.locator('.channel-item').filter({
    has: who.page.locator('.channel-name', { hasText: name }),
  });
  await expect(row).toBeVisible();
  const join = row.locator('.join-btn');
  if (await join.isVisible()) {
    const joined = who.page.waitForResponse(response => response.request().method() === 'POST'
      && new URL(response.url()).pathname.endsWith('/join'));
    await join.click(); expect((await joined).ok()).toBe(true);
  }
  await row.locator('.channel-select').click();
}
async function create(who: Actor, origin: string, name: string) {
  await who.page.getByRole('button', { name: 'Chat', exact: true }).click();
  await who.page.locator('.create-channel-btn').click();
  const dialog = who.page.getByRole('dialog', { name: 'Create Channel', exact: true });
  await dialog.getByPlaceholder('general').fill(name);
  await dialog.getByLabel('Type', { exact: true }).selectOption('Text');
  const created = who.page.waitForResponse(response => response.request().method() === 'POST'
    && new URL(response.url()).pathname === '/api/channels');
  await dialog.getByRole('button', { name: 'Create', exact: true }).click();
  expect((await created).ok()).toBe(true);
  await expect(dialog).toHaveCount(0);
  const channel = (await read<Channel[]>(who, origin, '/api/channels')).find(item => item.name === name);
  expect(Boolean(channel)).toBe(true);
  await select(who, name);
  return channel!;
}
async function send(who: Actor, origin: string, channel: Channel, marker: string) {
  await select(who, channel.name);
  await who.page.getByPlaceholder('Type a message...').fill(marker);
  await who.page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(bubble(who.page, marker).locator('.reply-btn')).toBeVisible();
  let message: Message | undefined;
  await expect.poll(async () => {
    message = (await read<Message[]>(who, origin, `/api/channels/${channel.channel_id}/messages?limit=100`))
      .find(item => item.content === marker);
    return Boolean(message);
  }).toBe(true);
  return message!;
}
async function evidence(info: TestInfo, value: Record<string, unknown>) {
  await info.attach('navigation-public-evidence', { contentType: 'application/json',
    body: Buffer.from(JSON.stringify(value, null, 2)) });
}
async function admin(who: Actor, section: string) {
  await who.page.locator('.admin-menu-btn').click();
  await who.page.locator('.admin-dropdown').getByRole('button', { name: section, exact: true }).click();
  await expect(who.page.getByRole('heading', { name: section, exact: true })).toBeVisible();
  await expect(who.page.locator('.admin-loading')).toHaveCount(0);
}
async function switchTo(who: Actor, summary: ServerSummary) {
  const hub = who.page.getByRole('navigation', { name: 'Your servers', exact: true });
  await hub.getByRole('button', { name: summary.label, exact: true }).click();
  await expect(who.page.locator('.status-bar .server-slug')).toHaveText(summary.slug);
  await ready(who);
}
async function importFile(who: Actor, bytes: Buffer, name: string) {
  const chooser = who.page.waitForEvent('filechooser');
  await who.page.getByRole('button', { name: 'Choose backup file', exact: true }).click();
  await (await chooser).setFiles({ name, mimeType: 'application/json', buffer: bytes });
  await expect(who.page.locator('.import-filename')).toHaveText(name);
  await who.page.getByRole('button', { name: 'Import', exact: true }).click();
}
async function interrupted(who: Actor, link: RealNetworkLink) {
  let held = false;
  who.page.on('response', response => {
    if (held || response.url() !== `${link.url}/api/public/server/summary` || !response.ok()) return;
    held = true;
    void response.finished().then(error => { if (!error) link.hold(); });
  });
  await who.page.locator('.add-server-btn').click();
  const dialog = who.page.getByRole('dialog', { name: 'Join a Server', exact: true });
  await dialog.getByLabel('Server URL', { exact: true }).fill(link.url);
  await dialog.getByRole('button', { name: 'Join Server', exact: true }).click();
  let placeholder: PublicServer | undefined;
  await expect.poll(async () => {
    placeholder = (await servers(who.page)).find(item => item.baseUrl === link.url && !item.identityId);
    return Boolean(placeholder);
  }, { timeout: 10_000 }).toBe(true);
  expect(held, 'The real server summary must precede transport interruption').toBe(true);
  await who.page.close(); link.cut();
  who.page = await who.context.newPage(); observe(who, who.page);
  await completeStartup(who.page);
  await expect(who.page.locator('.server-hub-icon.failed')).toHaveCount(1);
  return placeholder!;
}

test.describe.serial('real identity, multi-server and navigation actions', () => {
  test.beforeAll(async ({ browser }) => {
    expect(new URL(home).origin !== new URL(remote).origin, 'Two independently configured real servers are required').toBe(true);
    owner = await actor(browser);
    remoteOwner = await actor(browser, remote);
    await expect(owner.page.locator('.admin-menu-btn'), 'Use a fresh home server for founder registration').toBeVisible();
    await expect(remoteOwner.page.locator('.admin-menu-btn'), 'Use a fresh second server for founder registration').toBeVisible();
    homeSummary = await read<ServerSummary>(owner, home, '/api/public/server/summary', false);
    remoteSummary = await read<ServerSummary>(remoteOwner, remote, '/api/public/server/summary', false);
    expect(homeSummary.slug).not.toBe(remoteSummary.slug);
    expect(homeSummary.label).not.toBe(remoteSummary.label);
    homeChannel = await create(owner, home, `${run}-home`);
    remoteChannel = await create(remoteOwner, remote, `${run}-remote`);
  });
  test.afterAll(async () => {
    for (const link of links) link.resume();
    await Promise.all(actors.map(who => who.context.close()));
    await Promise.all(links.map(link => link.close()));
  });
  test.afterEach(() => {
    for (const who of actors) expect(who.errors, 'Uncaught browser errors').toEqual([]);
  });

  test('actual downloaded backup rejects an invalid file and restores the same operational identity in a clean browser', async ({ browser }, info) => {
    const original = (await identities(owner.page))[0];
    const before = await send(owner, home, homeChannel, `${run}-before-export`);
    const pending = owner.page.waitForEvent('download');
    await owner.page.getByRole('button', { name: 'Export', exact: true }).click();
    const download = await pending;
    expect(download.suggestedFilename()).toMatch(/^annex-identity-[a-zA-Z0-9_-]+\.json$/);
    const stream = await download.createReadStream();
    expect(Boolean(stream)).toBe(true);
    const chunks: Buffer[] = [];
    for await (const chunk of stream!) chunks.push(Buffer.from(chunk));
    const backup = Buffer.concat(chunks);
    for (const chunk of chunks) chunk.fill(0);
    await download.delete();
    const imported = await actor(browser, home, false);
    try {
      // Do not compare or attach the private JSON: boolean checks redact it even
      // when a check fails. The actual downloaded bytes drive the import below.
      const parsed = JSON.parse(backup.toString('utf8')) as Record<string, unknown>;
      expect(typeof parsed.sk === 'string' && parsed.sk.length > 0, 'Backup contains usable private key material').toBe(true);
      expect(parsed.pseudonymId === original.pseudonymId && parsed.commitmentHex === original.commitmentHex,
        'Downloaded backup belongs to the exporting identity').toBe(true);
      await owner.page.getByRole('button', { name: 'Logout', exact: true }).click();
      await expect(owner.page.getByRole('heading', { name: 'Create Your Identity', exact: true })).toBeVisible();
      await imported.page.goto('/');
      await expect(imported.page.getByRole('button', { name: 'Import', exact: true })).toBeDisabled();
      await importFile(imported, Buffer.from('{invalid-json'), 'invalid-backup.json');
      await expect(imported.page.getByRole('alert')).toHaveText('That file is not a usable Annex backup.');
      expect(await identities(imported.page)).toEqual([]);
      const verified = imported.page.waitForResponse(response => response.request().method() === 'POST'
        && new URL(response.url()).pathname === '/api/zk/verify-membership', { timeout: 150_000 });
      await importFile(imported, backup, 'annex-backup.json');
      await ready(imported);
      expect((await verified).ok(), 'Restoration must obtain fresh real membership credentials').toBe(true);
      const restored = (await identities(imported.page))[0];
      expect(restored.pseudonymId).toBe(original.pseudonymId);
      expect(restored.commitmentHex).toBe(original.commitmentHex);
      expect(restored.id).not.toBe(original.id);
      await select(imported, homeChannel.name);
      await expect(bubble(imported.page, before.content)).toBeVisible();
      const after = await send(imported, home, homeChannel, `${run}-after-import`);
      expect(after.sender_pseudonym).toBe(original.pseudonymId);
      await imported.page.reload(); await ready(imported); await select(imported, homeChannel.name);
      await expect(bubble(imported.page, after.content)).toBeVisible();
      await owner.context.close(); owner = imported;
      await evidence(info, { restoredPseudonym: restored.pseudonymId, freshLocalRecord: true,
        actualVerification: true, beforeMessageId: before.message_id, afterMessageId: after.message_id,
        privateBackupPersistedInReport: false });
    } finally { backup.fill(0); }
  });

  test('Logout shows the actual existing identity and resuming it preserves history and authorization', async () => {
    const info = test.info();
    const before = await identities(owner.page);
    const current = before.find(item => item.serverSlug === homeSummary.slug)!;
    await owner.page.getByRole('button', { name: 'Logout', exact: true }).click();
    await expect(owner.page.getByRole('heading', { name: 'Existing Identities', exact: true })).toBeVisible();
    await expect(owner.page.getByRole('button', { name: 'Chat', exact: true })).toHaveCount(0);
    await expect(owner.page.locator('.identity-option')).toHaveCount(before.length);
    await owner.page.locator('.identity-option').filter({ hasText: current.pseudonymId!.slice(0, 16) }).click();
    await ready(owner);
    expect(await identities(owner.page)).toEqual(before);
    const sent = await send(owner, home, homeChannel, `${run}-resumed`);
    expect(sent.sender_pseudonym).toBe(current.pseudonymId);
    await expect(owner.page.locator('.admin-menu-btn')).toBeVisible();
    await evidence(info, { resumedPseudonym: current.pseudonymId, persistedMessageId: sent.message_id, addedIdentities: 0 });
  });

  test('Audio settings persist a real preference; modal focus and every operator navigation entry work', async () => {
    const info = test.info();
    const audio = owner.page.getByRole('button', { name: /^Audio & Video Settings/ });
    await audio.click();
    const dialog = owner.page.getByRole('dialog', { name: 'Audio & Video Settings', exact: true });
    await expect(dialog.getByRole('combobox', { name: /^Input Device/ })).toBeVisible();
    await expect(dialog.getByRole('combobox', { name: /^Output Device/ })).toBeVisible();
    await expect(dialog.getByRole('combobox', { name: /^Camera/ })).toBeVisible();
    const slider = dialog.getByRole('slider');
    await slider.focus(); await slider.press('Home');
    for (let i = 0; i < 37; i++) await slider.press('ArrowRight');
    await expect(slider).toHaveValue('37');
    await expect(dialog.locator('.volume-value')).toHaveText('37%');
    const done = dialog.getByRole('button', { name: 'Done', exact: true });
    await done.focus(); await done.press('Tab');
    expect(await dialog.evaluate(element => element.contains(document.activeElement))).toBe(true);
    await done.click(); await expect(dialog).toHaveCount(0); await expect(audio).toBeFocused();
    expect(await owner.page.evaluate(() => JSON.parse(localStorage.getItem('annex:audioSettings') ?? '{}').outputVolume)).toBe(37);
    await owner.page.reload(); await ready(owner); await audio.click();
    await expect(dialog.getByRole('slider')).toHaveValue('37');
    await owner.page.keyboard.press('Escape'); await expect(dialog).toHaveCount(0); await expect(audio).toBeFocused();
    await owner.page.locator('.admin-menu-btn').click(); await expect(owner.page.locator('.admin-dropdown')).toBeVisible();
    await owner.page.getByRole('heading', { name: 'Annex', exact: true }).click();
    await expect(owner.page.locator('.admin-dropdown')).toHaveCount(0);
    const sections = ['Server Settings', 'Server Policy', 'Member Management', 'Channel Management', 'Federation Delivery'];
    for (const section of sections) {
      await admin(owner, section);
      await expect(owner.page.locator('.admin-dropdown')).toHaveCount(0);
    }
    const outbox = await read<{ entries: unknown[]; counts: Record<string, number> }>(owner, home, '/api/admin/federation/outbox?limit=50');
    expect(outbox.entries).toEqual([]);
    await expect(owner.page.locator('.outbox-empty')).toHaveText('Nothing has been queued for a federation peer yet.');
    await expect(owner.page.locator('.agreements-empty')).toHaveText('This server has no federation agreements.');
    for (const status of ['pending', 'failed', 'paused', 'delivered', '']) {
      const response = owner.page.waitForResponse(item => new URL(item.url()).pathname === '/api/admin/federation/outbox');
      if (status === '') await owner.page.getByLabel('Show', { exact: true }).selectOption('');
      else await owner.page.getByLabel('Show', { exact: true }).selectOption(status);
      expect((await response).ok()).toBe(true);
      await expect(owner.page.locator('.outbox-row')).toHaveCount(0);
    }
    await owner.page.getByRole('button', { name: 'Federation', exact: true }).click();
    await expect(owner.page.getByRole('heading', { name: /^Federation Peers/ })).toBeVisible();
    await expect(owner.page.getByText('No federation peers', { exact: true })).toBeVisible();
    expect((await read<{ peers: FederationPeer[] }>(owner, home, '/api/public/federation/peers', false)).peers).toEqual([]);
    await owner.page.getByRole('button', { name: 'Events', exact: true }).click();
    await expect(owner.page.locator('.event-log')).toBeVisible();
    await select(owner, homeChannel.name);
    await evidence(info, { outputVolume: 37, preferenceSurvivedReload: true, operatorSections: sections,
      emptyFederationComparedWithServer: true, hardwarePlaybackProven: false });
  });

  test('a genuinely registered agent is discoverable and its detail dialog matches actual negotiated capabilities', async () => {
    const info = test.info();
    const policy = await read<ServerPolicy>(owner, home, '/api/admin/policy');
    // Import the existing protocol harness, not application internals. It runs
    // real Groth16 registration and the server's actual VRP negotiation.
    const moduleUrl = new URL('../../../scripts/live/protocol.mjs', import.meta.url).href;
    const protocol = await import(moduleUrl) as {
      register: (url: string, role: number) => Promise<{ token: string; pseudonym: string }>;
      handshake: (policy: ServerPolicy) => unknown;
      request: (url: string, path: string, options: Record<string, unknown>) => Promise<{ alignment_status: string }>;
    };
    const registered = await protocol.register(home, 2);
    const negotiated = await protocol.request(home, '/api/vrp/agent-handshake', {
      method: 'POST', token: registered.token,
      body: { pseudonymId: registered.pseudonym, handshake: protocol.handshake(policy) },
    });
    expect(negotiated.alignment_status).toBe('Aligned');
    await owner.page.reload(); await ready(owner);
    const actual = await read<{ agents: AgentInfo[] }>(owner, home, '/api/public/agents', false);
    const agent = actual.agents.find(item => item.pseudonym_id === registered.pseudonym);
    expect(Boolean(agent)).toBe(true);
    const trigger = owner.page.locator('.agent-item').filter({ hasText: registered.pseudonym.slice(0, 12) });
    await expect(trigger).toBeVisible(); await trigger.click();
    const detail = owner.page.getByRole('dialog', { name: /^Agent:/ });
    await expect(detail.locator('dd').nth(0)).toHaveText(agent!.alignment_status);
    await expect(detail.locator('dd').nth(1)).toHaveText(agent!.transfer_scope);
    await expect(detail.locator('dd').nth(2)).toHaveText(agent!.reputation_score.toFixed(2));
    if (agent!.capabilities.length) await expect(detail.locator('dd').nth(3).locator('li')).toHaveText(agent!.capabilities);
    else await expect(detail.locator('dd').nth(3)).toHaveText('None declared');
    await detail.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(detail).toHaveCount(0); await expect(trigger).toBeFocused();
    await trigger.click(); await owner.page.keyboard.press('Escape');
    await expect(detail).toHaveCount(0); await expect(trigger).toBeFocused();
    await evidence(info, { agentPseudonym: agent!.pseudonym_id, alignment: agent!.alignment_status,
      scope: agent!.transfer_scope, capabilities: agent!.capabilities, actualModelInferenceProven: false });
  });

  test('joining and switching two actual servers preserves separate pseudonyms, histories and saved registrations', async () => {
    const info = test.info();
    const original = (await identities(owner.page)).find(item => item.serverSlug === homeSummary.slug)!;
    const remoteGreeting = await send(remoteOwner, remote, remoteChannel, `${run}-remote-greeting`);
    await owner.page.locator('.add-server-btn').click();
    const dialog = owner.page.getByRole('dialog', { name: 'Join a Server', exact: true });
    await dialog.getByLabel('Server URL', { exact: true }).fill(remote);
    await dialog.getByRole('button', { name: 'Join Server', exact: true }).click();
    await ready(owner);
    await expect(owner.page.locator('.status-bar .server-slug')).toHaveText(remoteSummary.slug);
    const saved = await servers(owner.page);
    expect(saved).toHaveLength(2);
    const records = await identities(owner.page);
    const alternate = records.find(item => item.serverSlug === remoteSummary.slug)!;
    expect(Boolean(alternate?.pseudonymId)).toBe(true);
    expect(alternate.id).not.toBe(original.id);
    expect(alternate.pseudonymId).not.toBe(original.pseudonymId);
    // Server hopping deliberately reuses the cryptographic key commitment.
    expect(alternate.commitmentHex).toBe(original.commitmentHex);
    expect(saved.find(item => item.slug === homeSummary.slug)?.identityId).toBe(original.id);
    expect(saved.find(item => item.slug === remoteSummary.slug)?.identityId).toBe(alternate.id);
    await expect(owner.page.locator('.admin-menu-btn')).toHaveCount(0);
    await expect(owner.page.locator('.channel-name').filter({ hasText: homeChannel.name })).toHaveCount(0);
    await select(owner, remoteChannel.name); await expect(bubble(owner.page, remoteGreeting.content)).toBeVisible();
    const sentRemote = await send(owner, remote, remoteChannel, `${run}-remote-identity`);
    expect(sentRemote.sender_pseudonym).toBe(alternate.pseudonymId);
    await expect(bubble(remoteOwner.page, sentRemote.content)).toBeVisible();
    expect((await read<Message[]>(remoteOwner, remote, `/api/channels/${remoteChannel.channel_id}/messages?limit=100`))
      .filter(item => item.message_id === sentRemote.message_id)).toHaveLength(1);
    await owner.page.reload(); await ready(owner);
    await expect(owner.page.locator('.status-bar .server-slug')).toHaveText(remoteSummary.slug);
    await select(owner, remoteChannel.name); await expect(bubble(owner.page, sentRemote.content)).toBeVisible();
    await switchTo(owner, homeSummary);
    await expect(owner.page.locator('.admin-menu-btn')).toBeVisible();
    await expect(owner.page.locator('.channel-name').filter({ hasText: remoteChannel.name })).toHaveCount(0);
    const sentHome = await send(owner, home, homeChannel, `${run}-home-identity`);
    expect(sentHome.sender_pseudonym).toBe(original.pseudonymId);
    await switchTo(owner, remoteSummary); await select(owner, remoteChannel.name);
    await expect(bubble(owner.page, sentRemote.content)).toBeVisible();
    await expect(bubble(owner.page, sentHome.content)).toHaveCount(0);
    expect((await servers(owner.page)).map(item => item.id).sort()).toEqual(saved.map(item => item.id).sort());
    await switchTo(owner, homeSummary);
    await evidence(info, { homePseudonym: original.pseudonymId, remotePseudonym: alternate.pseudonymId,
      distinctLocalRecords: true, sharedCommitmentByDesign: true,
      homeMessageId: sentHome.message_id, remoteMessageId: sentRemote.message_id, persistenceVerified: true });
  });

  test('genuine interrupted remote registration supports Remove and successful Retry after transport recovery', async ({ browser }, info) => {
    const who = await actor(browser);
    const link = await realNetworkLink(remote); links.push(link);
    const baseline = await servers(who.page);
    await test.step('Remove a placeholder left by closing a genuinely stalled registering tab', async () => {
      const placeholder = await interrupted(who, link);
      await who.page.getByRole('button', { name: 'Remove server', exact: true }).click();
      await expect(who.page.locator('.server-hub-icon.failed')).toHaveCount(0);
      expect((await servers(who.page)).some(item => item.id === placeholder.id)).toBe(false);
      expect((await servers(who.page)).map(item => item.id).sort()).toEqual(baseline.map(item => item.id).sort());
    });
    await test.step('Retry another real interrupted registration after restoring its network', async () => {
      link.resume(); await interrupted(who, link);
      link.resume();
      await who.page.getByRole('button', { name: 'Retry registration', exact: true }).click();
      await ready(who);
      await expect(who.page.locator('.status-bar .server-slug')).toHaveText(remoteSummary.slug);
      await expect(who.page.locator('.server-hub-icon.failed')).toHaveCount(0);
      const joined = (await servers(who.page)).find(item => item.baseUrl === link.url);
      expect(Boolean(joined?.identityId)).toBe(true);
      const marker = `${run}-retried-registration`;
      await send(who, link.url, remoteChannel, marker);
      await select(remoteOwner, remoteChannel.name); await expect(bubble(remoteOwner.page, marker)).toBeVisible();
    });
    await evidence(info, { interruptedActualTransport: true, removedPersistedPlaceholder: true,
      successfulRetryObservedByIndependentPeer: true });
  });

  test('real transport failures show member and federation errors and Retry reloads the live data', async ({ browser }, info) => {
    const link = await realNetworkLink(home); links.push(link);
    const who = await actor(browser, link.url);
    await who.page.getByRole('button', { name: 'Events', exact: true }).click();
    link.cut();
    await who.page.getByRole('button', { name: 'Chat', exact: true }).click();
    await expect(who.page.locator('.member-list-error')).toContainText('Could not load the member list');
    link.resume();
    const loaded = who.page.waitForResponse(response => new URL(response.url()).pathname === '/api/public/server/summary');
    await who.page.locator('.member-list-error').getByRole('button', { name: 'Retry', exact: true }).click();
    expect((await loaded).ok()).toBe(true);
    await expect(who.page.locator('.member-list-error')).toHaveCount(0);
    const summary = await read<ServerSummary>(who, link.url, '/api/public/server/summary', false);
    await expect(who.page.locator('.server-summary h3')).toHaveText(summary.label);
    link.cut();
    await who.page.getByRole('button', { name: 'Federation', exact: true }).click();
    await expect(who.page.locator('.federation-panel').getByRole('alert')).toContainText('Could not load federation peers');
    await expect(who.page.getByText('No federation peers', { exact: true })).toHaveCount(0);
    link.resume();
    const recovered = who.page.waitForResponse(response => new URL(response.url()).pathname === '/api/public/federation/peers');
    await who.page.locator('.federation-panel').getByRole('button', { name: 'Retry', exact: true }).click();
    expect((await recovered).ok()).toBe(true);
    await expect(who.page.locator('.federation-panel').getByRole('alert')).toHaveCount(0);
    await expect(who.page.getByText('No federation peers', { exact: true })).toBeVisible();
    await evidence(info, { actualTcpFailure: true, memberRetryLoadedServerData: true, federationRetryLoadedServerData: true });
  });
});

// Separate, explicitly opt-in source-bound definition for an existing real
// federation. It never manufactures peer rows or agreement state. A skipped
// test is an open acceptance item, not evidence that this surface passed.
test('existing genuine federation exposes peer metadata and normal Close/Escape controls', async ({ browser }, info) => {
  const url = process.env.ANNEX_NAV_FEDERATED_URL;
  test.skip(!url, 'Needs a separately established real federation node; core lane uses fresh independent nodes.');
  const who = await actor(browser, url!);
  try {
    const actual = await read<{ peers: FederationPeer[] }>(who, url!, '/api/public/federation/peers', false);
    expect(actual.peers.length, 'Configured federation node must have real accepted peers').toBeGreaterThan(0);
    await who.page.getByRole('button', { name: 'Federation', exact: true }).click();
    for (const peer of actual.peers) {
      const row = who.page.locator('.peer-item').filter({ has: who.page.locator('.peer-url', { hasText: peer.base_url }) });
      await expect(row).toBeVisible();
      await expect(row.locator('.peer-alignment')).toContainText(peer.alignment_status);
      await expect(row.locator('.peer-scope')).toContainText(peer.transfer_scope);
      const trigger = row.getByRole('button', { name: 'Explore', exact: true });
      await trigger.click();
      const detail = who.page.getByRole('dialog', { name: 'Upstream Federation', exact: true });
      const summary = await read<ServerSummary>(who, peer.base_url, '/api/public/server/summary', false);
      await expect(detail.locator('.peer-detail-label')).toHaveText(summary.label);
      await expect(detail.locator('.peer-detail-slug')).toHaveText(summary.slug);
      await expect(detail.locator('.stat-value')).toHaveText([
        String(summary.total_active_members), String(summary.channel_count), String(summary.federation_peer_count), String(summary.active_agent_count),
      ]);
      await detail.getByRole('button', { name: 'Close', exact: true }).click();
      await expect(detail).toHaveCount(0); await expect(trigger).toBeFocused();
      await trigger.click(); await who.page.keyboard.press('Escape');
      await expect(detail).toHaveCount(0); await expect(trigger).toBeFocused();
    }
    await evidence(info, { independentlyReadPeerUrls: actual.peers.map(item => item.base_url),
      genuinePeerDetailsCompared: true, federationJoinOrSeverProven: false });
  } finally { await who.context.close(); }
});
