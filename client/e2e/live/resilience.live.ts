import { test, expect, type Browser, type Page, type TestInfo } from '@playwright/test';
import jsQR from 'jsqr';
import { completeStartup } from '../helpers';
import { admin, baseURL, mutation, newActor, publicIdentities, read, select, send, type Actor } from './operator-helpers';
import { realNetworkLink, type RealNetworkLink } from './resilience-network';
import type { Channel, Message, MessageEdit } from '../../src/types';

const run = `resilience-${Date.now()}`;
const actors: Actor[] = [];
const links: RealNetworkLink[] = [];
let founder: Actor;
let member: Actor;
let memberLink: RealNetworkLink;

async function network() {
  const link = await realNetworkLink(baseURL);
  links.push(link);
  return link;
}
async function throughLink(browser: Browser, link: RealNetworkLink): Promise<Actor> {
  const context = await browser.newContext({ baseURL: link.url });
  const page = await context.newPage();
  const actor: Actor = { page, context, headers: {}, errors: [] };
  page.on('pageerror', error => actor.errors.push(error.message));
  page.on('request', request => {
    if (new URL(request.url()).origin !== link.url) return;
    const headers = request.headers();
    if (headers.authorization && headers['x-annex-zk-proof']) actor.headers = Object.fromEntries(
      Object.entries(headers).filter(([key]) => key === 'authorization' || key.startsWith('x-annex-')));
  });
  actors.push(actor);
  await completeStartup(page);
  return actor;
}
function bubble(page: Page, text: string) {
  return page.locator('.message').filter({ has: page.locator('.message-content').filter({ hasText: text }) });
}
async function history(actor: Actor, channelId: string) {
  return read<Message[]>(actor, `/api/channels/${channelId}/messages?limit=100`);
}
async function general() {
  return (await read<Channel[]>(founder, '/api/channels')).find(channel => channel.name === 'General')!;
}
async function receipt(info: TestInfo, value: Record<string, unknown>) {
  await info.attach('real-session-evidence', { body: Buffer.from(JSON.stringify(value, null, 2)), contentType: 'application/json' });
}
async function savedServers(page: Page) {
  return page.evaluate(async () => new Promise<Array<{ id: string; identityId: string; baseUrl: string }>>((resolve, reject) => {
    const opening = indexedDB.open('annex-servers');
    opening.onerror = () => reject(new Error('Saved-server database read failed'));
    opening.onsuccess = () => {
      const db = opening.result;
      if (!db.objectStoreNames.contains('servers')) { db.close(); resolve([]); return; }
      const request = db.transaction('servers', 'readonly').objectStore('servers').getAll();
      request.onerror = () => { db.close(); reject(new Error('Saved-server read failed')); };
      request.onsuccess = () => {
        resolve(request.result.map((item: { id: string; identityId: string; baseUrl: string }) => ({
          id: item.id, identityId: item.identityId, baseUrl: item.baseUrl,
        })));
        db.close();
      };
    };
  }));
}
async function create(name: string, channelType: 'Text' | 'Broadcast') {
  await founder.page.getByRole('button', { name: 'Chat', exact: true }).click();
  await founder.page.locator('.create-channel-btn').click();
  const dialog = founder.page.getByRole('dialog', { name: 'Create Channel', exact: true });
  await dialog.getByPlaceholder('general').fill(name);
  await dialog.getByLabel('Type', { exact: true }).selectOption(channelType);
  await mutation(founder, 'POST', '/api/channels', () => dialog.getByRole('button', { name: 'Create', exact: true }).click());
  await expect(dialog).toHaveCount(0);
  return (await read<Channel[]>(founder, '/api/channels')).find(channel => channel.name === name)!;
}

/** Produce a genuine interrupted-registration placeholder by stalling the real
 * transport only after the real summary arrived, then closing the registering
 * tab. No IndexedDB write, application-store injection, or invented response. */
async function interruptRegistration(actor: Actor, link: RealNetworkLink) {
  const page = actor.page;
  let held = false;
  const stall = (response: import('@playwright/test').Response) => {
    if (held || response.url() !== `${link.url}/api/public/server/summary` || !response.ok()) return;
    held = true;
    void response.finished().then(error => { if (!error) link.hold(); });
  };
  page.on('response', stall);
  await page.locator('.add-server-btn').click();
  const dialog = page.getByRole('dialog', { name: 'Join a Server', exact: true });
  await dialog.getByLabel('Server URL', { exact: true }).fill(link.url);
  await dialog.getByRole('button', { name: 'Join Server', exact: true }).click();
  let placeholder: { id: string; identityId: string; baseUrl: string } | undefined;
  await expect.poll(async () => {
    placeholder = (await savedServers(page)).find(server => server.baseUrl === link.url && !server.identityId);
    return Boolean(placeholder);
  }, { timeout: 10_000 }).toBe(true);
  expect(held, 'Real server summary must arrive before transport is stalled').toBe(true);
  await page.close();
  link.cut();
  actor.page = await actor.context.newPage();
  actor.page.on('pageerror', error => actor.errors.push(error.message));
  await completeStartup(actor.page);
  expect((await savedServers(actor.page)).find(server => server.id === placeholder!.id)?.identityId).toBe('');
  await expect(actor.page.locator('.server-hub-icon.failed')).toHaveCount(1);
  return placeholder!;
}

test.describe.serial('real server resilience and action recovery', () => {
  test.beforeAll(async ({ browser }) => {
    founder = await newActor(browser);
    actors.push(founder);
    await expect(founder.page.locator('.admin-menu-btn')).toBeVisible();
    memberLink = await network();
    member = await throughLink(browser, memberLink);
    await expect(member.page.locator('.admin-menu-btn')).toHaveCount(0);
    await select(founder, 'General');
    await select(member, 'General');
  });
  test.afterAll(async () => {
    for (const link of links) link.resume();
    await Promise.all(actors.map(actor => actor.context.close()));
    await Promise.all(links.map(link => link.close()));
  });
  test.afterEach(() => {
    for (const actor of actors) expect(actor.errors, 'Uncaught browser errors').toEqual([]);
  });

  test('actual dropped sockets reconnect, catch up once, and preserve an unsent draft', async () => {
    const info = test.info();
    const channel = await general();
    const anchor = `${run}-resume-anchor`;
    await send(founder, 'General', anchor);
    await expect(bubble(member.page, anchor)).toHaveCount(1);
    const draft = `${run}-preserved-draft`;
    await member.page.getByPlaceholder('Type a message...').fill(draft);
    await test.step('Cut actual TCP connections and observe disconnected UI', async () => {
      memberLink.cut();
      await expect(member.page.locator('.reconnection-banner.disconnected')).toBeVisible();
      await expect(member.page.getByPlaceholder('Type a message...')).toBeDisabled();
      await expect(member.page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
      expect((await history(founder, channel.channel_id)).some(message => message.content === draft)).toBe(false);
    });
    const missed = [`${run}-missed-1`, `${run}-missed-2`];
    for (const text of missed) await send(founder, 'General', text);
    await test.step('Restore actual transport and verify replay against independent sender history', async () => {
      memberLink.resume();
      await expect(member.page.locator('.reconnection-banner.disconnected')).toHaveCount(0, { timeout: 45_000 });
      for (const text of missed) await expect(bubble(member.page, text)).toHaveCount(1);
      await expect(member.page.getByPlaceholder('Type a message...')).toHaveValue(draft);
      await member.page.getByRole('button', { name: 'Send', exact: true }).click();
      await expect(bubble(founder.page, draft)).toHaveCount(1);
      await expect(bubble(member.page, draft).locator('.reply-btn')).toBeVisible();
      const persisted = await history(founder, channel.channel_id);
      for (const text of [anchor, ...missed, draft]) expect(persisted.filter(message => message.content === text)).toHaveLength(1);
    });
    await receipt(info, { network: 'real TCP sockets cut and reopened', channelId: channel.channel_id, missed, draft });
  });

  test('real edited badge loads persisted revisions after an actual request failure and Retry', async () => {
    const info = test.info();
    const channel = await general();
    const original = `${run}-revision-original`;
    const revised = `${run}-revision-current`;
    const message = await send(founder, 'General', original);
    await expect(bubble(member.page, original)).toBeVisible();
    const source = bubble(founder.page, original);
    await source.locator('.edit-btn').click();
    await source.locator('.message-edit-input').fill(revised);
    await source.locator('.msg-edit-save').click();
    const remote = bubble(member.page, revised);
    await expect(remote.locator('.edited-badge')).toBeVisible();
    memberLink.cut();
    await expect(member.page.locator('.reconnection-banner.disconnected')).toBeVisible();
    await remote.locator('.edited-badge').click();
    await expect(remote.locator('.edit-history-error')).toContainText('Could not load edit history');
    await expect(remote.locator('.edit-history-empty')).toHaveCount(0);
    memberLink.resume();
    await expect(member.page.locator('.reconnection-banner.disconnected')).toHaveCount(0, { timeout: 45_000 });
    await remote.locator('.edit-history-error').getByRole('button', { name: 'Retry', exact: true }).click();
    await expect(remote.locator('.edit-history-content')).toHaveText([original, revised]);
    const edits = await read<MessageEdit[]>(founder, `/api/channels/${channel.channel_id}/messages/${message.message_id}/edits`);
    expect(edits.map(edit => edit.old_content)).toEqual([original]);
    expect((await history(founder, channel.channel_id)).find(item => item.message_id === message.message_id)?.content).toBe(revised);
    await remote.locator('.edited-badge').click();
    await expect(remote.locator('.edit-history')).toHaveCount(0);
    await remote.locator('.edited-badge').click();
    await expect(remote.locator('.edit-history-content')).toHaveText([original, revised]);
    await receipt(info, { messageId: message.message_id, storedRevisionCount: edits.length, original, revised });
  });

  test('offline edit and confirmed delete report refusal and retain the actual original', async () => {
    const info = test.info();
    const channel = await general();
    const original = `${run}-offline-author-original`;
    const proposed = `${run}-offline-author-unsaved`;
    const message = await send(member, 'General', original);
    await expect(bubble(founder.page, original)).toBeVisible();
    const own = bubble(member.page, original);
    await own.locator('.edit-btn').click();
    await own.locator('.message-edit-input').fill(proposed);
    memberLink.cut();
    await expect(member.page.locator('.reconnection-banner.disconnected')).toBeVisible();
    await own.locator('.msg-edit-save').click();
    await expect(member.page.locator('.message-action-error')).toContainText('Edit not saved');
    await expect(bubble(member.page, proposed)).toHaveCount(0);
    await expect(bubble(member.page, original)).toBeVisible();
    await member.page.locator('.message-action-error').getByRole('button', { name: 'Dismiss', exact: true }).click();
    await own.locator('.delete-btn').click();
    await expect(own.locator('.delete-confirm-text')).toHaveText('Confirm?');
    await own.locator('.delete-btn').click();
    await expect(member.page.locator('.message-action-error')).toContainText('Message not deleted');
    await member.page.locator('.message-action-error').getByRole('button', { name: 'Dismiss', exact: true }).click();
    const persisted = (await history(founder, channel.channel_id)).find(item => item.message_id === message.message_id)!;
    expect(persisted.content).toBe(original);
    expect(persisted.deleted_at).toBeNull();
    memberLink.resume();
    await expect(member.page.locator('.reconnection-banner.disconnected')).toHaveCount(0, { timeout: 45_000 });
    await expect(bubble(founder.page, original)).toBeVisible();
    await receipt(info, { messageId: persisted.message_id, content: persisted.content, deletedAt: persisted.deleted_at });
  });

  test('actual Broadcast refusal supports dismissal and retry after a real capability grant', async () => {
    const info = test.info();
    const name = `${run}-broadcast`;
    const channel = await create(name, 'Broadcast');
    await select(founder, name);
    await expect(founder.page.getByText('No messages to display.', { exact: true })).toBeVisible();
    await select(member, name);
    const rejected = [`${run}-discard-refused`, `${run}-retry-refused`];
    for (const text of rejected) {
      await member.page.getByPlaceholder('Type a message...').fill(text);
      await member.page.getByRole('button', { name: 'Send', exact: true }).click();
      await expect(bubble(member.page, text).locator('.failed-status')).toBeVisible();
      await expect(member.page.locator('.composer-error')).toContainText(/broadcast|moderator/i);
      expect((await history(founder, channel.channel_id)).some(item => item.content === text)).toBe(false);
    }
    await bubble(member.page, rejected[0]).locator('.msg-dismiss-btn').click();
    await expect(bubble(member.page, rejected[0])).toHaveCount(0);
    await bubble(member.page, rejected[1]).locator('.msg-retry-btn').click();
    await expect(bubble(member.page, rejected[1]).locator('.failed-status')).toBeVisible();
    expect(await history(founder, channel.channel_id)).toHaveLength(0);
    const pseudonym = (await publicIdentities(member)).find(identity => identity.pseudonymId)!.pseudonymId!;
    await admin(founder, 'Member Management');
    const memberRow = founder.page.locator('.member-row').filter({ has: founder.page.locator(`.member-pseudonym[title="${pseudonym}"]`) });
    await mutation(founder, 'PUT', `/api/admin/members/${pseudonym}/capabilities`, () => memberRow.getByLabel('Moderator', { exact: true }).check());
    await expect(memberRow.getByLabel('Moderator', { exact: true })).toBeChecked();
    await bubble(member.page, rejected[1]).locator('.msg-retry-btn').click();
    await expect(bubble(member.page, rejected[1]).locator('.reply-btn')).toBeVisible();
    await expect.poll(async () => (await history(founder, channel.channel_id)).filter(item => item.content === rejected[1]).length).toBe(1);
    await mutation(founder, 'PUT', `/api/admin/members/${pseudonym}/capabilities`, () => memberRow.getByLabel('Moderator', { exact: true }).uncheck());
    await select(founder, name);
    await expect(bubble(founder.page, rejected[1])).toHaveCount(1);
    await receipt(info, { channelId: channel.channel_id, refused: rejected, confirmedAfterGrant: rejected[1], discardedPersisted: false });
  });

  test('first startup rejects malformed and actually unreachable addresses, then registers on the real server', async ({ browser }, info) => {
    const actor = await newActor(browser, false);
    actors.push(actor);
    const link = await network();
    link.cut();
    await actor.page.goto('/');
    await actor.page.getByRole('button', { name: 'Create New Identity', exact: true }).click();
    const input = actor.page.getByPlaceholder('annex.example.com');
    await expect(input).toBeVisible();
    await input.fill('http://');
    await actor.page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(actor.page.getByRole('alert')).toContainText('Could not use "http://" as a server address');
    await input.fill(link.url);
    const failed = actor.page.waitForEvent('requestfailed', request => request.url() === `${link.url}/api/public/server/summary`);
    await actor.page.getByRole('button', { name: 'Connect', exact: true }).click();
    await failed;
    await expect(actor.page.getByRole('alert')).toContainText(`Could not reach server at ${link.url}`);
    await input.fill(baseURL);
    await actor.page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(actor.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible({ timeout: 150_000 });
    const marker = `${run}-startup-recovered`;
    await select(founder, 'General');
    await send(actor, 'General', marker);
    await expect(bubble(founder.page, marker)).toBeVisible();
    const saved = await savedServers(actor.page);
    expect(saved.some(server => server.baseUrl === baseURL && Boolean(server.identityId))).toBe(true);
    await actor.page.reload();
    await expect(input).toHaveValue(baseURL);
    await actor.page.getByRole('button', { name: 'Connect', exact: true }).click();
    await expect(actor.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible({ timeout: 150_000 });
    await select(actor, 'General');
    await expect(bubble(actor.page, marker)).toBeVisible();
    await receipt(info, { malformedRejected: true, transportFailed: true, registeredServer: baseURL, marker });
  });

  test('server dialog failure preserves saved servers; interrupted registration exposes real Retry and Remove', async ({ browser }, info) => {
    const actor = await newActor(browser);
    actors.push(actor);
    const link = await network();
    link.cut();
    const baseline = await savedServers(actor.page);
    await actor.page.locator('.add-server-btn').click();
    const dialog = actor.page.getByRole('dialog', { name: 'Join a Server', exact: true });
    await expect(dialog.getByRole('button', { name: 'Join Server', exact: true })).toBeDisabled();
    await dialog.getByLabel('Server URL', { exact: true }).fill('http://');
    await dialog.getByRole('button', { name: 'Join Server', exact: true }).click();
    await expect(dialog.getByRole('alert')).toContainText('Could not use "http://" as a server address');
    await dialog.getByLabel('Server URL', { exact: true }).fill(link.url);
    await dialog.getByRole('button', { name: 'Join Server', exact: true }).click();
    await expect(dialog.getByRole('alert')).toContainText(`Could not reach ${link.url}`);
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    expect(await savedServers(actor.page)).toEqual(baseline);

    await test.step('Close a tab during a real stalled registration, then remove its persisted placeholder', async () => {
      link.resume();
      const placeholder = await interruptRegistration(actor, link);
      await actor.page.getByRole('button', { name: 'Remove server', exact: true }).click();
      await expect(actor.page.locator('.server-hub-icon.failed')).toHaveCount(0);
      expect((await savedServers(actor.page)).some(server => server.id === placeholder.id)).toBe(false);
      expect((await savedServers(actor.page)).map(server => server.id)).toEqual(baseline.map(server => server.id));
    });
    await test.step('Retry a second actual interrupted registration while its connection remains down', async () => {
      link.resume();
      await interruptRegistration(actor, link);
      await actor.page.getByRole('button', { name: 'Retry registration', exact: true }).click();
      await expect(actor.page.locator('.server-switch-error')).toContainText('Could not retry');
      await expect(actor.page.locator('.server-switch-error')).toContainText(link.url);
      await actor.page.locator('.server-switch-error').getByRole('button', { name: 'Dismiss', exact: true }).click();
      await expect(actor.page.locator('.server-switch-error')).toHaveCount(0);
      expect((await savedServers(actor.page)).map(server => server.id)).toEqual(baseline.map(server => server.id));
      await select(founder, 'General');
      const marker = `${run}-hub-after-retry`;
      // Replacement pages have the same origin and real persisted identity.
      // Independent founder delivery proves the restored session is usable.
      await select(actor, 'General');
      await actor.page.getByPlaceholder('Type a message...').fill(marker);
      await actor.page.getByRole('button', { name: 'Send', exact: true }).click();
      await expect(bubble(founder.page, marker)).toBeVisible();
    });
    await receipt(info, { savedServerIdsPreserved: baseline.map(server => server.id), placeholderTrigger: 'actual stalled registration followed by tab close', removed: true, retryFailureDismissed: true });
  });

  test('real tooltip supports keyboard focus, hover, blur, and narrow-viewport bounds', async () => {
    const info = test.info();
    await founder.page.getByRole('button', { name: 'Chat', exact: true }).click();
    const before = (await read<Channel[]>(founder, '/api/channels')).map(channel => channel.channel_id).sort();
    const bounds: Array<{ width: number; x: number; right: number }> = [];
    for (const width of [1280, 390]) {
      await founder.page.setViewportSize({ width, height: 900 });
      await founder.page.locator('.create-channel-btn').click();
      const dialog = founder.page.getByRole('dialog', { name: 'Create Channel', exact: true });
      const tip = dialog.locator('.info-tip');
      const text = await tip.getAttribute('aria-label');
      await tip.focus();
      const tooltip = dialog.getByRole('tooltip');
      await expect(tooltip).toHaveText(text!);
      await expect.poll(async () => {
        const box = await tooltip.boundingBox();
        return Boolean(box && box.x >= 7 && box.x + box.width <= width - 7);
      }).toBe(true);
      const box = (await tooltip.boundingBox())!;
      bounds.push({ width, x: box.x, right: box.x + box.width });
      await tip.press('Tab');
      await expect(tooltip).toHaveCount(0);
      await tip.hover();
      await expect(tooltip).toHaveText(text!);
      await dialog.getByRole('heading').hover();
      await expect(tooltip).toHaveCount(0);
      await expect(dialog.getByRole('checkbox')).not.toBeChecked();
      await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    }
    await founder.page.setViewportSize({ width: 1280, height: 900 });
    expect((await read<Channel[]>(founder, '/api/channels')).map(channel => channel.channel_id).sort()).toEqual(before);
    await receipt(info, { measuredTooltipBounds: bounds, keyboardAndHover: true, channelMutation: false });
  });

  test('actual transfer QR scans at 360 by 740 and its normal Done control is reachable', async () => {
    await founder.page.setViewportSize({ width: 360, height: 740 });
    await founder.page.getByRole('button', { name: 'Link', exact: true }).click();
    await founder.page.getByRole('button', { name: /Link a New Device/ }).click();
    const dialog = founder.page.getByRole('dialog', { name: 'Device Linking', exact: true });
    const transfer = await dialog.getByLabel('Transfer code', { exact: true }).inputValue();
    const graphic = dialog.getByRole('img', { name: 'Encrypted identity transfer QR code', exact: true });
    await graphic.scrollIntoViewIfNeeded();
    const pixels = await graphic.evaluate(async element => {
      const svg = element.querySelector('svg');
      if (!svg) throw new Error('Actual rendered QR SVG is absent');
      const bounds = svg.getBoundingClientRect();
      const width = Math.round(bounds.width);
      const height = Math.round(bounds.height);
      const canvas = document.createElement('canvas');
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext('2d');
      if (!context) throw new Error('Actual displayed QR rasterization unavailable');
      const url = URL.createObjectURL(new Blob([new XMLSerializer().serializeToString(svg)], { type: 'image/svg+xml' }));
      try {
        const image = new Image();
        image.src = url;
        await image.decode();
        context.fillStyle = '#fff';
        context.fillRect(0, 0, width, height);
        context.drawImage(image, 0, 0, width, height);
        return { width, height, x: bounds.x, right: bounds.right, data: Array.from(context.getImageData(0, 0, width, height).data) };
      } finally { URL.revokeObjectURL(url); }
    });
    expect(pixels.x).toBeGreaterThanOrEqual(0);
    expect(pixels.right).toBeLessThanOrEqual(360);
    const scanned = jsQR(new Uint8ClampedArray(pixels.data), pixels.width, pixels.height);
    // Never include transfer ciphertext, pairing code, or identity keys in diffs.
    expect(Boolean(scanned), 'Displayed mobile QR must decode independently').toBe(true);
    expect(scanned?.data === transfer, 'Decoded mobile QR must equal the actual transfer payload').toBe(true);
    await dialog.getByRole('button', { name: 'Done', exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await expect(founder.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible();
    await founder.page.setViewportSize({ width: 1280, height: 900 });
    await receipt(test.info(), { viewport: { width: 360, height: 740 }, displayedQr: { width: pixels.width, height: pixels.height }, independentlyScanned: true, normalDoneClick: true });
  });
});
