import { test, expect } from '@playwright/test';
import { completeStartup } from '../helpers';
import type { PublicEvent, ServerPolicy } from '../../src/types';
import {
  type Actor, admin, baseURL, fillPolicy, mutation, newActor, publicIdentities,
  read, reload, savePolicy, select, send, startUnregistered, toggles,
} from './operator-helpers';

/** Real UI writes, independent registered viewers, authenticated readback. No fixtures. */
const run = `operator-${Date.now()}`;
const actors: Actor[] = [];
let founder: Actor;
let member: Actor;
let hidden: Actor;
let originalPolicy: ServerPolicy;

async function identityDialog(who: Actor) {
  await who.page.getByRole('button', { name: 'Identity', exact: true }).click();
  await expect(who.page.getByRole('dialog')).toBeVisible();
  return who.page.getByRole('dialog');
}
async function visibleNames(who: Actor) {
  return (await read<{ usernames: Record<string, string> }>(who, '/api/usernames/visible')).usernames;
}
async function evidence(name: string, who: Actor) {
  const path = test.info().outputPath(`${name}.png`);
  await who.page.screenshot({ path });
  await test.info().attach(name, { path, contentType: 'image/png' });
}
async function finishExistingStartup(who: Actor) {
  const deadline = Date.now() + 150_000;
  while (Date.now() < deadline) {
    if (await who.page.getByRole('button', { name: 'Chat', exact: true }).isVisible()) return;
    const proceed = who.page.getByRole('button', { name: 'Continue', exact: true });
    if (await proceed.isVisible()) await proceed.click();
    const retry = who.page.getByRole('button', { name: 'Retry', exact: true });
    if (await retry.isVisible()) await retry.click();
    await who.page.waitForTimeout(500);
  }
  await expect(who.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible();
}

test.describe('Unstubbed operator, privacy and recovery actions', () => {
  test.describe.configure({ mode: 'serial' });
  test.beforeAll(async ({ browser }) => {
    founder = await newActor(browser); actors.push(founder);
    await expect(founder.page.locator('.admin-menu-btn'), 'Fresh database required').toBeVisible();
    member = await newActor(browser); actors.push(member);
    hidden = await newActor(browser); actors.push(hidden);
    originalPolicy = await read<ServerPolicy>(founder, '/api/admin/policy');
  });
  test.afterAll(async () => { for (const who of actors) await who.context.close(); });
  test.afterEach(() => { for (const who of actors) expect(who.errors, 'Uncaught browser errors').toEqual([]); });

  test('server settings save actual name, logo and normalized local URL across reload', async () => {
    await admin(founder, 'Server Settings');
    const serverName = founder.page.locator('label').filter({ hasText: 'Server Name' }).locator('input');
    await serverName.fill('');
    await expect(founder.page.getByRole('button', { name: 'Save server name', exact: true })).toBeDisabled();
    await serverName.fill(run);
    await mutation(founder, 'PATCH', '/api/admin/server', () => founder.page.getByRole('button', { name: 'Save server name', exact: true }).click());
    expect((await read<{ label: string }>(founder, '/api/admin/server')).label).toBe(run);
    expect((await read<{ label: string }>(member, '/api/public/server/summary')).label).toBe(run);
    const previous = (await read<{ public_url: string }>(founder, '/api/admin/server')).public_url;
    await founder.page.getByLabel('Public URL', { exact: true }).fill('invalid-without-scheme');
    await founder.page.getByRole('button', { name: 'Save public URL', exact: true }).click();
    await expect(founder.page.getByRole('alert')).toContainText('must start with');
    expect((await read<{ public_url: string }>(founder, '/api/admin/server')).public_url).toBe(previous);
    await founder.page.getByLabel('Public URL', { exact: true }).fill(`${baseURL}///`);
    await mutation(founder, 'PUT', '/api/admin/public-url', () => founder.page.getByRole('button', { name: 'Save public URL', exact: true }).click());
    expect((await read<{ public_url: string }>(founder, '/api/admin/server')).public_url).toBe(baseURL);
    const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAHElEQVQI12P4//8/w38GIAXDIBKE0DHxgljNBAAO9TXL0Y4OHwAAAABJRU5ErkJggg==', 'base64');
    const uploaded = await mutation(founder, 'POST', '/api/admin/server/image', async () => {
      const picker = founder.page.waitForEvent('filechooser');
      await founder.page.getByRole('button', { name: 'Upload Image', exact: true }).click();
      await (await picker).setFiles({ name: 'acceptance-logo.png', mimeType: 'image/png', buffer: png });
    });
    const image = await uploaded.json() as { url: string };
    expect(image.url).toBeTruthy();
    await expect.poll(() => founder.page.locator('.server-image-preview').evaluate((element: HTMLImageElement) => element.complete && element.naturalWidth > 0)).toBe(true);
    await reload(founder);
    await admin(founder, 'Server Settings');
    await expect(serverName).toHaveValue(run);
    await expect(founder.page.getByLabel('Public URL', { exact: true })).toHaveValue(baseURL);
    await expect.poll(() => founder.page.locator('.server-image-preview').evaluate((element: HTMLImageElement) => element.complete && element.naturalWidth > 0)).toBe(true);
    const publicImage = await read<{ image_url: string | null }>(member, '/api/public/server/image');
    expect(publicImage.image_url).toBe(image.url);
    const bytes = await member.context.request.get(new URL(image.url, baseURL).href);
    expect(bytes.ok()).toBe(true);
    expect(bytes.headers()['content-type']).toContain('image/');
    // Change Image must also drive a real picker/upload, with a second
    // persisted upload id even when this test uses the same harmless pixels.
    const changed = await mutation(founder, 'POST', '/api/admin/server/image', async () => {
      const picker = founder.page.waitForEvent('filechooser');
      await founder.page.getByRole('button', { name: 'Change Image', exact: true }).click();
      await (await picker).setFiles({ name: 'replacement-logo.png', mimeType: 'image/png', buffer: png });
    });
    const replacement = await changed.json() as { url: string };
    expect((await read<{ image_url: string | null }>(member, '/api/public/server/image')).image_url).toBe(replacement.url);
    const health = await read<{ state: string; writes_blocked: boolean }>(founder, '/api/admin/storage');
    expect(health.state).toBe('healthy');
    expect(health.writes_blocked).toBe(false);
    await expect(founder.page.locator('.storage-gate-badge')).toHaveText('healthy');
    await evidence('persisted-server-settings', founder);
  });

  test('every policy input persists, toggles take effect, and tag removal persists', async () => {
    await admin(founder, 'Server Policy');
    const changed: ServerPolicy = {
      ...originalPolicy, access_mode: 'public', agent_min_alignment_score: 0.07,
      max_members: 1000, default_retention_days: 14,
      federation_enabled: true, voice_enabled: true, usernames_enabled: true,
      allow_reflection_summaries: true, allow_full_knowledge: true,
      images_enabled: true, videos_enabled: true, files_enabled: true,
      max_image_size_mb: 7, max_video_size_mb: 19, max_file_size_mb: 13,
      rate_limit: { registration_limit: 300, verification_limit: 600, default_limit: 3000 },
      agent_required_capabilities: [...originalPolicy.agent_required_capabilities, `${run}-capability`],
      principles: [...originalPolicy.principles, `${run} preserve user choice`],
      prohibited_actions: [...originalPolicy.prohibited_actions, `${run} leak private content`],
    };
    await fillPolicy(founder, changed);
    await savePolicy(founder);
    expect(await read<ServerPolicy>(founder, '/api/admin/policy')).toEqual(changed);
    await reload(founder); await admin(founder, 'Server Policy');
    await expect(founder.page.getByLabel(/^Access Mode/)).toHaveValue('public');
    await expect(founder.page.locator('.tag-item', { hasText: `${run}-capability` })).toBeVisible();
    for (const [, label] of toggles) await expect(founder.page.getByLabel(new RegExp(`^${label}`))).toBeChecked();
    const disabled = { ...changed };
    for (const [key, label] of toggles) { await founder.page.getByLabel(new RegExp(`^${label}`)).uncheck(); disabled[key] = false; }
    await savePolicy(founder);
    expect(await read<ServerPolicy>(founder, '/api/admin/policy')).toEqual(disabled);
    expect((await read<{ voice_enabled: boolean; policy_enabled: boolean }>(member, '/api/voice/config-status')).policy_enabled).toBe(false);
    expect(await visibleNames(member)).toEqual({});
    // Restore service switches through the same controls. Removed tags must
    // disappear in persisted readback, not merely from local form state.
    const restored = { ...originalPolicy, usernames_enabled: true, images_enabled: true };
    await fillPolicy(founder, restored); await savePolicy(founder);
    expect(await read<ServerPolicy>(founder, '/api/admin/policy')).toEqual(restored);
    await reload(founder); await admin(founder, 'Server Policy');
    await expect(founder.page.locator('.tag-item', { hasText: run })).toHaveCount(0);
    await evidence('policy-controls-persisted', founder);
  });

  test('knowledge-sharing controls persist and genuine agent re-handshakes negotiate the selected scope', async () => {
    const info = test.info();
    const before = await read<ServerPolicy>(founder, '/api/admin/policy');
    const protocolUrl = new URL('../../../scripts/live/protocol.mjs', import.meta.url).href;
    const protocol = await import(protocolUrl) as {
      register: (url: string, role: number) => Promise<{ token: string; pseudonym: string }>;
      handshake: (policy: ServerPolicy) => unknown;
      request: (url: string, route: string, options: Record<string, unknown>) => Promise<{
        alignment_status: string; transfer_scope: string; handshakeRef: string;
      }>;
    };
    const agent = await protocol.register(baseURL, 2);
    const receipts: Array<{ summaries: boolean; full: boolean; scope: string; handshakeRef: string }> = [];
    await admin(founder, 'Server Policy');
    try {
      for (const [summaries, full, scope] of [
        [true, false, 'ReflectionSummariesOnly'],
        [false, true, 'FullKnowledgeBundle'],
        [false, false, 'NoTransfer'],
        [true, false, 'ReflectionSummariesOnly'],
      ] as const) {
        await founder.page.getByRole('checkbox', { name: 'Allow Reflection Summaries', exact: true }).setChecked(summaries);
        await founder.page.getByRole('checkbox', { name: 'Allow Full Knowledge Bundles', exact: true }).setChecked(full);
        await savePolicy(founder);
        const actual = await read<ServerPolicy>(founder, '/api/admin/policy');
        expect(actual.allow_reflection_summaries).toBe(summaries);
        expect(actual.allow_full_knowledge).toBe(full);
        await reload(founder); await admin(founder, 'Server Policy');
        await expect(founder.page.getByRole('checkbox', { name: 'Allow Reflection Summaries', exact: true })).toBeChecked({ checked: summaries });
        await expect(founder.page.getByRole('checkbox', { name: 'Allow Full Knowledge Bundles', exact: true })).toBeChecked({ checked: full });
        const report = await protocol.request(baseURL, '/api/vrp/agent-handshake', {
          method: 'POST', token: agent.token,
          body: { pseudonymId: agent.pseudonym, handshake: protocol.handshake(actual) },
        });
        expect(report.alignment_status).toBe('Aligned');
        expect(report.transfer_scope).toBe(scope);
        expect(report.handshakeRef).toBeTruthy();
        expect(receipts.some(item => item.handshakeRef === report.handshakeRef)).toBe(false);
        const profile = await read<{ transfer_scope: string; handshakeRef: string }>(founder, `/api/agents/${agent.pseudonym}`);
        expect(profile.transfer_scope).toBe(scope);
        expect(profile.handshakeRef).toBe(report.handshakeRef);
        receipts.push({ summaries, full, scope, handshakeRef: report.handshakeRef });
      }
      await info.attach('actual-knowledge-policy-negotiation', { contentType: 'application/json',
        body: Buffer.from(JSON.stringify({ agentPseudonym: agent.pseudonym, receipts })) });
    } finally {
      await founder.page.getByRole('checkbox', { name: 'Allow Reflection Summaries', exact: true }).setChecked(before.allow_reflection_summaries);
      await founder.page.getByRole('checkbox', { name: 'Allow Full Knowledge Bundles', exact: true }).setChecked(before.allow_full_knowledge);
      await savePolicy(founder);
    }
  });

  test('password rejects a wrong credential, accepts the right one, and invite-only refuses no-invite registration', async ({ browser }) => {
    await admin(founder, 'Server Policy');
    await founder.page.getByLabel(/^Access Mode/).selectOption('password');
    const password = `local-acceptance-${crypto.randomUUID()}`;
    await founder.page.getByPlaceholder('Enter server password...').fill(password);
    await savePolicy(founder);
    const guest = await newActor(browser, false); actors.push(guest);
    await startUnregistered(guest);
    await expect(guest.page.getByPlaceholder('Enter server password', { exact: true })).toBeVisible();
    const rejected = guest.page.waitForResponse(response => new URL(response.url()).pathname === '/api/registry/register' && response.request().method() === 'POST');
    await guest.page.getByPlaceholder('Enter server password', { exact: true }).fill('wrong-test-password');
    await guest.page.getByRole('button', { name: 'Join Server', exact: true }).click();
    expect((await rejected).status()).toBe(403);
    await expect(guest.page.getByRole('button', { name: 'Chat', exact: true })).toHaveCount(0);
    await expect(guest.page.getByRole('alert')).toBeVisible();
    await guest.page.getByRole('button', { name: 'Retry', exact: true }).click();
    const continueButton = guest.page.getByRole('button', { name: 'Continue', exact: true });
    if (await continueButton.isVisible()) await continueButton.click();
    await expect(guest.page.getByPlaceholder('Enter server password', { exact: true })).toBeVisible();
    await guest.page.getByPlaceholder('Enter server password', { exact: true }).fill(password);
    await guest.page.getByRole('button', { name: 'Join Server', exact: true }).click();
    await expect(guest.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible({ timeout: 150_000 });
    const message = await send(guest, 'General', `${run}-password-admitted`);
    expect(message.sender_pseudonym).toBe((await publicIdentities(guest))[0].pseudonymId);
    await founder.page.getByLabel(/^Access Mode/).selectOption('invite_only');
    await savePolicy(founder);
    const denied = await newActor(browser, false); actors.push(denied);
    const inviteRefusal = denied.page.waitForResponse(response => new URL(response.url()).pathname === '/api/registry/register' && response.request().method() === 'POST');
    await startUnregistered(denied);
    expect((await inviteRefusal).status()).toBe(403);
    await expect(denied.page.getByRole('alert')).toContainText(/invite/i);
    expect((await publicIdentities(denied)).every(item => item.pseudonymId === null)).toBe(true);
    await founder.page.getByLabel(/^Access Mode/).selectOption('public');
    await savePolicy(founder);
    // The same genuinely refused identity can retry after admission reopens.
    await completeStartup(denied.page);
    await expect(denied.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible();
  });

  test('ordinary member username visibility grants, revocation and deletion reach only authorized viewers', async () => {
    const owner = (await publicIdentities(member))[0].pseudonymId!;
    const viewer = (await publicIdentities(founder))[0].pseudonymId!;
    let dialog = await identityDialog(member);
    const username = `${run}-name`.slice(-32);
    await dialog.getByPlaceholder('Enter your display name...').fill(username);
    await mutation(member, 'PUT', '/api/profile/username', () => dialog.getByRole('button', { name: 'Save Username', exact: true }).click());
    expect((await visibleNames(member))[owner]).toBe(username);
    expect((await visibleNames(founder))[owner]).toBeUndefined();
    expect((await visibleNames(hidden))[owner]).toBeUndefined();
    // This is intentionally an ordinary member's dialog. A moderator-only
    // member-list endpoint cannot substitute for functioning user privacy UI.
    const viewerRow = dialog.locator('.member-row').filter({ has: dialog.locator(`.member-pseudonym[title="${viewer}"]`) });
    await expect(viewerRow).toBeVisible();
    await mutation(member, 'POST', '/api/profile/username/grant', () => viewerRow.getByRole('button', { name: 'Grant', exact: true }).click());
    await expect(viewerRow.getByRole('button', { name: 'Revoke', exact: true })).toBeVisible();
    expect((await read<{ grantees: string[] }>(member, '/api/profile/username/grants')).grantees).toContain(viewer);
    expect((await visibleNames(founder))[owner]).toBe(username);
    expect((await visibleNames(hidden))[owner]).toBeUndefined();
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    const body = `${run}-visible-name-message`;
    await send(member, 'General', body);
    await reload(founder); await select(founder, 'General');
    await expect(founder.page.locator('.message', { hasText: body })).toContainText(username);
    await reload(hidden); await select(hidden, 'General');
    await expect(hidden.page.locator('.message', { hasText: body })).not.toContainText(username);
    await reload(member); dialog = await identityDialog(member);
    await expect(dialog.getByPlaceholder('Enter your display name...')).toHaveValue(username);
    const revokeRow = dialog.locator('.member-row').filter({ has: dialog.locator(`.member-pseudonym[title="${viewer}"]`) });
    await mutation(member, 'DELETE', `/api/profile/username/grant/${viewer}`, () => revokeRow.getByRole('button', { name: 'Revoke', exact: true }).click());
    expect((await visibleNames(founder))[owner]).toBeUndefined();
    await mutation(member, 'DELETE', '/api/profile/username', () => dialog.getByRole('button', { name: 'Remove Username', exact: true }).click());
    expect((await visibleNames(member))[owner]).toBeUndefined();
    expect((await read<{ grantees: string[] }>(member, '/api/profile/username/grants')).grantees).toEqual([]);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await reload(founder); await select(founder, 'General');
    await expect(founder.page.locator('.message', { hasText: body })).not.toContainText(username);
    await evidence('username-removed-at-independent-viewer', founder);
  });

  test('social recovery refuses insufficient and duplicate shards then restores the same operational identity', async ({ browser }) => {
    const identity = (await publicIdentities(member))[0];
    await member.page.getByRole('button', { name: 'Recovery', exact: true }).click();
    let dialog = member.page.getByRole('dialog');
    await dialog.getByRole('button', { name: /Set Up Recovery/ }).click();
    await expect(dialog.locator('.guardian-entry')).toHaveCount(5);
    await dialog.getByRole('button', { name: 'Generate Shards', exact: true }).click();
    await expect(dialog.getByRole('alert')).toContainText('Name all 5 guardians');
    for (let i = 1; i <= 5; i++) await dialog.getByPlaceholder(`Guardian ${i} name`, { exact: true }).fill(`Controlled guardian ${i}`);
    await dialog.getByRole('button', { name: 'Generate Shards', exact: true }).click();
    await expect(dialog.locator('.shard-item')).toHaveCount(5);
    await dialog.getByRole('button', { name: 'Done', exact: true }).click();
    await expect(dialog.getByRole('alert')).toContainText('have not been copied');
    const shards: string[] = [];
    try {
      await member.page.bringToFront();
      for (let i = 0; i < 5; i++) {
        const shard = dialog.locator('.shard-item').nth(i);
        await shard.getByRole('button', { name: 'Copy', exact: true }).click();
        await expect(shard.getByRole('button', { name: 'Copied!', exact: true })).toBeVisible();
        shards.push(await member.page.evaluate(() => navigator.clipboard.readText()));
      }
      expect(new Set(shards).size).toBe(5);
      await dialog.getByRole('button', { name: 'Done', exact: true }).click();
      await expect(dialog).toHaveCount(0);
      const recovered = await newActor(browser, false); actors.push(recovered);
      await recovered.page.goto('/');
      await recovered.page.getByRole('button', { name: 'Recover Identity', exact: true }).click();
      dialog = recovered.page.getByRole('dialog');
      const fields = dialog.getByPlaceholder('Paste the shard your guardian sent', { exact: true });
      await fields.nth(0).fill(shards[0]); await fields.nth(1).fill(shards[1]);
      await dialog.getByRole('button', { name: 'Reconstruct Key', exact: true }).click();
      await expect(dialog.getByRole('alert')).toContainText('2 of the 3 shards needed');
      await fields.nth(2).fill(shards[1]);
      await dialog.getByRole('button', { name: 'Reconstruct Key', exact: true }).click();
      await expect(dialog.getByRole('alert')).toContainText('same shard was entered twice');
      await fields.nth(2).fill(shards[2]);
      await dialog.getByRole('button', { name: 'Reconstruct Key', exact: true }).click();
      await expect(dialog.getByText('Key reconstructed successfully!', { exact: true })).toBeVisible();
      await dialog.getByRole('button', { name: 'Import Recovered Key', exact: true }).click();
      await expect.poll(async () => (await publicIdentities(recovered)).some(item => item.commitmentHex === identity.commitmentHex)).toBe(true);
      const done = recovered.page.getByRole('dialog').getByRole('button', { name: 'Done', exact: true });
      if (await done.isVisible()) await done.click();
      await finishExistingStartup(recovered);
      const sent = await send(recovered, 'General', `${run}-recovered-identity-operational`);
      expect(sent.sender_pseudonym).toBe(identity.pseudonymId);
      await reload(recovered);
      await select(recovered, 'General');
      await expect(recovered.page.locator('.message-content', { hasText: `${run}-recovered-identity-operational` })).toBeVisible();
      await evidence('recovered-identity-operational-after-reload', recovered);
    } finally {
      // Clear only the test's own copied secret if it is still on the clipboard.
      await member.page.bringToFront();
      await member.page.evaluate(async values => {
        if (values.includes(await navigator.clipboard.readText())) await navigator.clipboard.writeText('');
      }, shards).catch(() => {});
      shards.fill('');
    }
  });

  test('real message search results and each event-domain filter match persisted server data', async () => {
    const body = `${run}-searchable-message`;
    await send(member, 'General', body);
    await founder.page.getByRole('button', { name: 'Chat', exact: true }).click();
    await select(founder, 'General');
    await founder.page.getByRole('button', { name: 'Search messages', exact: true }).click();
    const search = founder.page.getByRole('search');
    await search.getByRole('textbox', { name: 'Search messages', exact: true }).fill(body);
    await expect(search.locator('.search-no-results')).toHaveCount(0);
    const found = founder.page.waitForResponse(response => new URL(response.url()).pathname === '/api/messages/search');
    await search.getByRole('button', { name: 'Search', exact: true }).click();
    expect((await found).ok()).toBe(true);
    await expect(search.getByRole('option', { name: new RegExp(body) })).toBeVisible();
    await search.getByRole('option', { name: new RegExp(body) }).click();
    await expect(search).toHaveCount(0);
    await expect(founder.page.locator('.message-content', { hasText: body })).toBeVisible();
    await founder.page.keyboard.press('Control+f');
    await search.getByRole('textbox', { name: 'Search messages', exact: true }).fill(`${run}-absent`);
    await search.getByRole('button', { name: 'Search', exact: true }).click();
    await expect(search.locator('.search-no-results')).toHaveText('No messages found');
    await search.getByRole('textbox', { name: 'Search messages', exact: true }).fill('new-unsubmitted-query');
    await expect(search.locator('.search-no-results')).toHaveCount(0);
    await search.getByRole('textbox', { name: 'Search messages', exact: true }).fill(body);
    await founder.context.setOffline(true);
    try {
      await search.getByRole('button', { name: 'Search', exact: true }).click();
      await expect(search.locator('.search-error')).toContainText('Search failed');
      await expect(search.locator('.search-no-results')).toHaveCount(0);
    } finally { await founder.context.setOffline(false); }
    const retrySearch = founder.page.waitForResponse(response => new URL(response.url()).pathname === '/api/messages/search');
    await search.locator('.search-error').getByRole('button', { name: 'Retry', exact: true }).click();
    expect((await retrySearch).ok()).toBe(true);
    await expect(search.getByRole('option', { name: new RegExp(body) })).toBeVisible();
    await search.getByRole('button', { name: 'Close search', exact: true }).click();
    await expect(search).toHaveCount(0);
    await founder.page.keyboard.press('Control+f');
    await founder.page.keyboard.press('Escape');
    await expect(search).toHaveCount(0);
    await founder.page.getByRole('button', { name: 'Events', exact: true }).click();
    for (const domain of ['IDENTITY', 'PRESENCE', 'FEDERATION', 'AGENT', 'MODERATION', 'ALL']) {
      const loaded = founder.page.waitForResponse(response => {
        const url = new URL(response.url());
        return url.pathname === '/api/public/events' && (url.searchParams.get('domain') ?? 'ALL') === domain;
      });
      await founder.page.getByLabel('Filter events by domain', { exact: true }).selectOption(domain);
      const response = await loaded;
      expect(response.ok()).toBe(true);
      const payload = await response.json() as PublicEvent[] | { events: PublicEvent[] };
      const events = Array.isArray(payload) ? payload : payload.events;
      await expect(founder.page.locator('.event-row')).toHaveCount(events.length);
      expect(events.every(item => domain === 'ALL' || item.domain === domain)).toBe(true);
      const shownTypes = await founder.page.locator('.event-row .event-col-type').allTextContents();
      expect(shownTypes).toEqual(events.map(item => item.event_type));
    }
    const refreshed = founder.page.waitForResponse(response => new URL(response.url()).pathname === '/api/public/events');
    await founder.page.getByRole('button', { name: 'Refresh', exact: true }).click();
    expect((await refreshed).ok()).toBe(true);
    await expect(founder.page.getByRole('button', { name: 'Refresh', exact: true })).toBeEnabled();
    await founder.context.setOffline(true);
    try {
      await founder.page.getByRole('button', { name: 'Refresh', exact: true }).click();
      await expect(founder.page.locator('.event-log-error')).toContainText('Could not load events');
      await expect(founder.page.locator('.event-log-empty')).toHaveCount(0);
    } finally { await founder.context.setOffline(false); }
    const retryLoaded = founder.page.waitForResponse(response => new URL(response.url()).pathname === '/api/public/events');
    await founder.page.locator('.event-log-error').getByRole('button', { name: 'Retry', exact: true }).click();
    expect((await retryLoaded).ok()).toBe(true);
    await expect(founder.page.locator('.event-log-error')).toHaveCount(0);
    await evidence('real-audit-events', founder);
  });
});
