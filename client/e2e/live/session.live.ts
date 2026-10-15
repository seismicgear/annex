import { test, expect, type Browser, type BrowserContext, type Page } from '@playwright/test';
import jsQR from 'jsqr';
import { completeStartup } from '../helpers';
import type { Channel, Message, MessageEdit } from '../../src/types';

/**
 * Real application acceptance: no route interception, synthetic API responses,
 * direct database seeding, application-store writes, or auth bypasses.
 *
 * All product mutations use visible controls. Readbacks repeat the running
 * app's authenticated GET requests and verify persisted server data. Each
 * message mutation is also observed by a separate participant.
 */
interface Actor {
  context: BrowserContext;
  page: Page;
  headers: Record<string, string>;
  errors: string[];
  socketAttempts: number;
}

const baseURL = process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000';
const run = `live-${Date.now()}`;
const actors: Actor[] = [];
let founder: Actor;
let member: Actor;

async function newActor(browser: Browser): Promise<Actor> {
  const context = await browser.newContext({ baseURL });
  const page = await context.newPage();
  const actor: Actor = { context, page, headers: {}, errors: [], socketAttempts: 0 };
  page.on('websocket', socket => {
    if (new URL(socket.url()).pathname === '/ws') actor.socketAttempts += 1;
  });
  page.on('pageerror', error => actor.errors.push(error.message));
  page.on('request', request => {
    // Copy only credentials actually issued to this real browser session.
    // They remain in memory and are never attached to the evidence report.
    if (new URL(request.url()).origin !== new URL(baseURL).origin) return;
    const headers = request.headers();
    if (headers.authorization && headers['x-annex-zk-proof']) {
      actor.headers = Object.fromEntries(Object.entries(headers).filter(([key]) =>
        key === 'authorization' || key.startsWith('x-annex-')));
    }
  });
  actors.push(actor);
  return actor;
}

async function read<T>(actor: Actor, route: string): Promise<T> {
  await expect.poll(() => Boolean(actor.headers.authorization), {
    message: 'The application must have obtained genuine session credentials',
  }).toBe(true);
  const response = await actor.context.request.get(new URL(route, baseURL).href, {
    headers: actor.headers,
  });
  expect(response.status(), `GET ${route}`).toBe(200);
  return response.json() as Promise<T>;
}

async function channel(actor: Actor, name: string): Promise<Channel> {
  const channels = await read<Channel[]>(actor, '/api/channels');
  const found = channels.find(item => item.name === name);
  expect(found, `The server must persist channel ${name}`).toBeDefined();
  return found!;
}

async function history(actor: Actor, channelId: string): Promise<Message[]> {
  return read<Message[]>(actor, `/api/channels/${encodeURIComponent(channelId)}/messages?limit=100`);
}

function row(page: Page, name: string) {
  return page.locator('.channel-item').filter({
    has: page.locator('.channel-name', { hasText: new RegExp(`^${name}$`) }),
  });
}

async function select(actor: Actor, name: string) {
  const entry = row(actor.page, name);
  await expect(entry).toBeVisible();
  if (await entry.locator('.join-btn').isVisible()) {
    const joined = actor.page.waitForResponse(response =>
      response.request().method() === 'POST' && new URL(response.url()).pathname.endsWith('/join'));
    await entry.locator('.join-btn').click();
    expect((await joined).ok()).toBe(true);
    await expect(entry.locator('.leave-btn')).toBeVisible();
  }
  await entry.locator('.channel-select').click();
  await expect(entry).toHaveClass(/active/);
}

async function reload(actor: Actor) {
  await actor.page.reload();
  await expect(actor.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible({ timeout: 150_000 });
  await expect(actor.page.locator('.channel-list')).toBeVisible();
}

async function create(name: string, type: string, federated = false, actor = founder) {
  const page = actor.page;
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await page.locator('.create-channel-btn').click();
  const dialog = page.getByRole('dialog');
  await dialog.getByPlaceholder('general').fill(name);
  await dialog.locator('select').selectOption(type);
  await dialog.getByPlaceholder('What this channel is about').fill(`Acceptance ${type} ${federated ? 'Federated' : 'Local'}`);
  await dialog.locator('input[type="checkbox"]').setChecked(federated);
  const created = page.waitForResponse(response =>
    response.request().method() === 'POST' && new URL(response.url()).pathname === '/api/channels');
  await dialog.getByRole('button', { name: 'Create', exact: true }).click();
  expect((await created).status()).toBe(200);
  await expect(dialog).toBeHidden();
  const persisted = await channel(actor, name);
  expect(persisted.channel_type).toBe(type);
  expect(persisted.federation_scope).toBe(federated ? 'Federated' : 'Local');
  return persisted;
}

async function send(actor: Actor, channelId: string, text: string): Promise<Message> {
  await actor.page.getByPlaceholder('Type a message...').fill(text);
  await actor.page.getByRole('button', { name: 'Send', exact: true }).click();
  const bubble = actor.page.locator('.message').filter({ has: actor.page.locator('.message-content', { hasText: text }) });
  await expect(bubble).toBeVisible();
  // A reply button is only rendered after the server confirms the message.
  await expect(bubble.locator('.reply-btn')).toBeVisible();
  await expect(bubble).not.toHaveClass(/pending|failed/);
  let persisted: Message | undefined;
  await expect.poll(async () => {
    persisted = (await history(actor, channelId)).find(item => item.content === text);
    return persisted?.message_id;
  }).toBeTruthy();
  return persisted!;
}

async function removeChannel(name: string, channelId: string) {
  const page = founder.page;
  await page.locator('.admin-menu-btn').click();
  await page.getByRole('button', { name: 'Channel Management', exact: true }).click();
  const item = page.locator('.channel-manager-item', { hasText: name });
  await item.getByRole('button', { name: 'Delete', exact: true }).click();
  let dialog = page.getByRole('dialog');
  await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
  expect((await channel(founder, name)).channel_id).toBe(channelId);
  await item.getByRole('button', { name: 'Delete', exact: true }).click();
  dialog = page.getByRole('dialog');
  const deleted = page.waitForResponse(response =>
    response.request().method() === 'DELETE' && new URL(response.url()).pathname === `/api/channels/${channelId}`);
  await dialog.getByRole('button', { name: 'Delete channel', exact: true }).click();
  expect((await deleted).ok()).toBe(true);
  await expect(item).toHaveCount(0);
  expect((await read<Channel[]>(founder, '/api/channels')).some(item => item.channel_id === channelId)).toBe(false);
  await page.getByRole('button', { name: 'Chat', exact: true }).click();
  await expect(row(page, name)).toHaveCount(0);
}

async function publicIdentity(page: Page) {
  return page.evaluate(async () => {
    const records = await new Promise<Array<{ id: string; commitmentHex: string; pseudonymId: string | null }>>((resolve, reject) => {
      const opening = indexedDB.open('annex-identity');
      opening.onerror = () => reject(new Error('Could not read identity storage'));
      opening.onsuccess = () => {
        const db = opening.result;
        if (!db.objectStoreNames.contains('identities')) { db.close(); resolve([]); return; }
        const request = db.transaction('identities', 'readonly').objectStore('identities').getAll();
        request.onerror = () => { db.close(); reject(new Error('Could not read identities')); };
        request.onsuccess = () => {
          // Do not export secret keys, transfer codes, proofs or bearer tokens.
          resolve(request.result.map((record: { id: string; commitmentHex: string; pseudonymId: string | null }) => ({
            id: record.id, commitmentHex: record.commitmentHex, pseudonymId: record.pseudonymId,
          })));
          db.close();
        };
      };
    });
    return records;
  });
}

async function evidence(name: string, actor = founder) {
  const info = test.info();
  await actor.page.screenshot({ path: info.outputPath(`${name}.png`), fullPage: true });
  await info.attach(name, { path: info.outputPath(`${name}.png`), contentType: 'image/png' });
}

test.describe('Independent live sessions on a fresh Annex server', () => {
  test.describe.configure({ mode: 'serial' });

  test.beforeAll(async ({ browser }) => {
    test.setTimeout(360_000);
    founder = await newActor(browser);
    await completeStartup(founder.page);
    await expect(founder.page.locator('.admin-menu-btn'), 'Fresh database required: first registrant must be moderator').toBeVisible();
    member = await newActor(browser);
    await completeStartup(member.page);
    await expect(member.page.locator('.admin-menu-btn')).toHaveCount(0);
    await expect(member.page.locator('.create-channel-btn')).toHaveCount(0);
    const first = await publicIdentity(founder.page);
    const second = await publicIdentity(member.page);
    expect(first[0].commitmentHex).not.toBe(second[0].commitmentHex);
    expect(first[0].pseudonymId).not.toBe(second[0].pseudonymId);
  });

  test.afterAll(async () => {
    for (const actor of actors) await actor.context.close();
  });

  test.afterEach(() => {
    for (const actor of actors) expect(actor.errors, 'Uncaught browser errors').toEqual([]);
  });

  test('device transfer rejects a wrong code, preserves identity and works after reload', async ({ browser }) => {
    await select(founder, 'General');
    const sourceChannel = await channel(founder, 'General');
    const sourceBody = `${run}-before-device-transfer`;
    await send(founder, sourceChannel.channel_id, sourceBody);
    const linked = await newActor(browser);
    const source = (await publicIdentity(founder.page))[0];
    const { code, payload } = await test.step('Generate and independently scan the actual displayed transfer QR', async () => {
    await founder.page.getByRole('button', { name: 'Link', exact: true }).click();
    await founder.page.getByRole('button', { name: /Link a New Device/ }).click();
    const transfer = await founder.page.getByLabel('Transfer code', { exact: true }).inputValue();
    const code = (await founder.page.locator('.pairing-code-value').innerText()).trim();
    expect(code).toMatch(/^\d{6}$/);
    expect(transfer.length).toBeGreaterThan(100);
    const pixels = await founder.page.getByRole('img', { name: 'Encrypted identity transfer QR code' }).evaluate(async element => {
      const svg = element.querySelector('svg');
      if (!svg) throw new Error('Rendered transfer QR SVG missing');
      const bounds = svg.getBoundingClientRect();
      const width = Math.round(bounds.width);
      const height = Math.round(bounds.height);
      const canvas = document.createElement('canvas');
      canvas.width = width;
      canvas.height = height;
      const context = canvas.getContext('2d');
      if (!context) throw new Error('QR rasterization unavailable');
      const url = URL.createObjectURL(new Blob([new XMLSerializer().serializeToString(svg)], { type: 'image/svg+xml' }));
      try {
        const image = new Image();
        image.src = url;
        await image.decode();
        context.fillStyle = '#fff';
        context.fillRect(0, 0, width, height);
        context.drawImage(image, 0, 0, width, height);
        return { width, height, data: Array.from(context.getImageData(0, 0, width, height).data) };
      } finally { URL.revokeObjectURL(url); }
    });
    // Independently decode the actual rendered graphic at its displayed size.
    // Keep encrypted transfer material out of report diffs and attachments.
    const scanned = jsQR(new Uint8ClampedArray(pixels.data), pixels.width, pixels.height);
    expect(Boolean(scanned), 'The displayed transfer QR must be independently scannable').toBe(true);
    expect(scanned?.data === transfer, 'QR scan must equal the application transfer payload').toBe(true);
    return { code, payload: scanned!.data };
    });

    await test.step('Reject an incorrect pairing code without creating an identity', async () => {
    await linked.page.goto('/');
    await linked.page.getByRole('button', { name: 'Link from Another Device', exact: true }).click();
    await linked.page.getByRole('button', { name: /Receive from Another Device/ }).click();
    await linked.page.getByPlaceholder('Paste the encoded data here...').fill(payload);
    const wrongCode = `${code[0] === '0' ? '1' : '0'}${code.slice(1)}`;
    await linked.page.getByPlaceholder('000000').fill(wrongCode);
    await linked.page.getByRole('button', { name: 'Import Identity', exact: true }).click();
    await expect(linked.page.getByRole('alert')).toBeVisible();
    expect(await publicIdentity(linked.page)).toHaveLength(0);
    });
    await test.step('Import the independently scanned payload with its real pairing code', async () => {
    await linked.page.getByPlaceholder('000000').fill(code);
    await linked.page.getByRole('button', { name: 'Import Identity', exact: true }).click();
    // Successful import may advance the app and unmount the dialog immediately.
    await expect.poll(async () => (await publicIdentity(linked.page)).length).toBe(1);
    const imported = (await publicIdentity(linked.page))[0];
    expect(imported.id).not.toBe(source.id);
    expect(imported.commitmentHex).toBe(source.commitmentHex);
    expect(imported.pseudonymId).toBe(source.pseudonymId);
    const success = linked.page.locator('.device-link-success');
    if (await success.isVisible()) await success.getByRole('button', { name: 'Continue', exact: true }).click();
    await founder.page.getByRole('button', { name: 'Done', exact: true }).click();
    });
    await test.step('Authenticate the transferred identity, send to an independent peer and retain it after reload', async () => {
    await completeStartup(linked.page);
    await expect(founder.page.getByText('This identity is active on another device.', { exact: true })).toBeVisible();
    await select(linked, 'General');
    await select(member, 'General');
    const general = await channel(linked, 'General');
    const body = `${run}-linked-identity-message`;
    const message = await send(linked, general.channel_id, body);
    expect(message.sender_pseudonym).toBe(source.pseudonymId);
    await expect(member.page.locator('.message-content', { hasText: body })).toBeVisible();
    await reload(linked);
    await select(linked, 'General');
    expect((await publicIdentity(linked.page))[0].commitmentHex).toBe(source.commitmentHex);
    expect((await history(linked, general.channel_id)).find(item => item.message_id === message.message_id)?.content).toBe(body);
    await evidence('device-link-operational-after-reload', linked);
    });

    await test.step('Keep both devices open, preserve readable history and require explicit session transfer', async () => {
      await expect(founder.page.locator('.message-content', { hasText: sourceBody })).toBeVisible();
      await expect(founder.page.getByText('Connection lost — reconnecting...', { exact: true })).toHaveCount(0);
      await expect(founder.page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
      await expect(founder.page.locator('.edit-btn').first()).toBeDisabled();
      const founderAttempts = founder.socketAttempts;
      // Reload must retain the displaced state. Wait beyond the maximum 30 s
      // reconnection delay while the other device remains open and connected.
      await reload(founder);
      await expect(founder.page.getByText('This identity is active on another device.', { exact: true })).toBeVisible();
      await founder.page.waitForTimeout(31_500);
      expect(founder.socketAttempts, 'Displaced and reloaded device must not retry automatically').toBe(founderAttempts);
      await expect(linked.page.locator('.ws-indicator')).toHaveClass(/\bconnected\b/);
      await founder.page.getByRole('button', { name: 'Reconnect here', exact: true }).click();
      await expect(founder.page.locator('.ws-indicator')).toHaveClass(/\bconnected\b/);
      await expect(linked.page.getByText('This identity is active on another device.', { exact: true })).toBeVisible();
      await expect(linked.page.getByRole('button', { name: 'Send', exact: true })).toBeDisabled();
      const linkedAttempts = linked.socketAttempts;
      await linked.page.waitForTimeout(31_500);
      expect(linked.socketAttempts, 'Previously active linked device must remain quiet after explicit transfer').toBe(linkedAttempts);
      await select(founder, 'General');
      const general = await channel(founder, 'General');
      const body = `${run}-ownership-returned`;
      await send(founder, general.channel_id, body);
      await expect(member.page.locator('.message-content', { hasText: body })).toBeVisible();
      await evidence('explicit-session-transfer-displaced-device', linked);
      await evidence('explicit-session-transfer-active-device', founder);
    });
  });

  test('message edit, reply and confirmed deletion reach another member and persisted history', async () => {
    const name = `${run}-chat`;
    const created = await create(name, 'Text');
    await select(founder, name);
    await reload(member);
    await select(member, name);
    const original = `${run}-original`;
    const edited = `${run}-edited`;
    const sent = await send(founder, created.channel_id, original);
    // Only one authored message exists in this newly created channel. Keep
    // the locator independent of body text while the edit form replaces it.
    const own = founder.page.locator('.message.self');
    await expect(member.page.locator('.message-content', { hasText: original })).toBeVisible();
    await expect(member.page.locator('.message', { hasText: original }).locator('.edit-btn')).toHaveCount(0);
    await own.hover();
    await own.locator('.edit-btn').click();
    await own.getByLabel('Edit message').fill('cancelled edit must never persist');
    await own.getByRole('button', { name: 'Cancel', exact: true }).click();
    expect((await history(member, created.channel_id)).find(item => item.message_id === sent.message_id)?.content).toBe(original);
    await own.hover();
    await own.locator('.edit-btn').click();
    await own.getByLabel('Edit message').fill(edited);
    await own.getByRole('button', { name: 'Save', exact: true }).click();
    await expect(member.page.locator('.message-content', { hasText: edited })).toBeVisible();
    const updated = (await history(member, created.channel_id)).find(item => item.message_id === sent.message_id);
    expect(updated?.content).toBe(edited);
    expect(updated?.edited_at).toBeTruthy();
    const edits = await read<MessageEdit[]>(founder, `/api/channels/${created.channel_id}/messages/${sent.message_id}/edits`);
    expect(edits.some(edit => edit.old_content === original)).toBe(true);

    const remote = member.page.locator('.message', { hasText: edited });
    await remote.hover();
    await remote.locator('.reply-btn').click();
    await member.page.getByRole('button', { name: 'Cancel reply', exact: true }).click();
    await expect(member.page.locator('.reply-bar')).toHaveCount(0);
    await remote.locator('.reply-btn').click();
    const replyBody = `${run}-reply`;
    const reply = await send(member, created.channel_id, replyBody);
    expect(reply.reply_to_message_id).toBe(sent.message_id);
    await expect(founder.page.locator('.message', { hasText: replyBody }).locator('.reply-context')).toContainText(edited);

    // Delete the just-edited original within the actual UI's edit/delete window.
    const changed = founder.page.locator('.message').filter({ has: founder.page.locator('.message-content', { hasText: edited }) });
    await changed.hover();
    await changed.locator('.delete-btn').click();
    await expect(changed.locator('.delete-btn.confirming')).toBeVisible();
    expect((await history(member, created.channel_id)).find(item => item.message_id === sent.message_id)?.deleted_at).toBeFalsy();
    await changed.locator('.delete-btn.confirming').click();
    await expect(member.page.locator('.message-deleted-text')).toHaveCount(1);
    const deleted = (await history(member, created.channel_id)).find(item => item.message_id === sent.message_id);
    expect(deleted?.deleted_at).toBeTruthy();
    expect(deleted?.content).toBe('');
    await reload(member);
    await select(member, name);
    await expect(member.page.locator('.message-deleted-text')).toHaveCount(1);
    expect((await history(member, created.channel_id)).find(item => item.message_id === reply.message_id)?.reply_to_message_id).toBe(sent.message_id);
    await evidence('message-delete-persists-on-independent-member', member);
    await removeChannel(name, created.channel_id);
    await reload(member);
    await expect(row(member.page, name)).toHaveCount(0);
  });

  test('all five types in both scopes persist and delete; membership respects agent-only access', async () => {
    test.setTimeout(600_000);
    for (const type of ['Text', 'Voice', 'Hybrid', 'Agent', 'Broadcast']) {
      for (const federated of [false, true]) {
        const name = `${run}-${type.toLowerCase()}-${federated ? 'fed' : 'local'}`;
        const created = await create(name, type, federated);
        await reload(founder);
        expect((await channel(founder, name)).channel_type).toBe(type);
        await reload(member);
        if (type === 'Agent') {
          const refused = member.page.waitForResponse(response =>
            response.request().method() === 'POST' && new URL(response.url()).pathname === `/api/channels/${created.channel_id}/join`);
          await row(member.page, name).locator('.join-btn').click();
          expect((await refused).status()).toBe(403);
          await expect(row(member.page, name).locator('.channel-action-error')).toBeVisible();
          expect((await channel(member, name)).is_member).toBe(false);
          await removeChannel(name, created.channel_id);
          await reload(member);
          await expect(row(member.page, name)).toHaveCount(0);
          continue;
        }
        await select(founder, name);
        await select(member, name);
        const entry = row(member.page, name);
        await entry.locator('.leave-btn').click();
        await expect(entry.locator('.join-btn')).toBeVisible();
        await reload(member);
        await expect(row(member.page, name).locator('.join-btn')).toBeVisible();
        await select(member, name);
        if (type === 'Broadcast') {
          const announcement = await send(founder, created.channel_id, `${name}-announcement`);
          await expect(member.page.locator('.message-content', { hasText: announcement.content })).toBeVisible();
          // A real non-moderator attempt must be visibly refused and must not persist.
          const denied = `${name}-unauthorized-post`;
          await member.page.getByPlaceholder('Type a message...').fill(denied);
          await member.page.getByRole('button', { name: 'Send', exact: true }).click();
          await expect(member.page.locator('.composer-error')).toBeVisible();
          expect((await history(founder, created.channel_id)).some(item => item.content === denied)).toBe(false);
        }
        await removeChannel(name, created.channel_id);
        await reload(member);
        await expect(row(member.page, name)).toHaveCount(0);
      }
    }
    await evidence('all-channel-types-and-scopes-deleted');
  });

  test('attachment cancel, real image upload and remote retrieval survive reload', async () => {
    const name = `${run}-attachments`;
    const created = await create(name, 'Text');
    await select(founder, name);
    await reload(member);
    await select(member, name);
    // Actual small PNG, posted through the real multipart upload endpoint.
    const bytes = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAQAAAAECAYAAACp8Z5+AAAAHElEQVQI12P4//8/w38GIAXDIBKE0DHxg' +
      'ljNBAAO9TXL0Y4OHwAAAABJRU5ErkJggg==', 'base64');
    const file = { name: `${run}.png`, mimeType: 'image/png', buffer: bytes };
    await founder.page.locator('.message-input input[type="file"]').setInputFiles(file);
    await expect(founder.page.locator('.image-preview-bar')).toBeVisible();
    await founder.page.getByTitle('Remove attachment', { exact: true }).click();
    await expect(founder.page.locator('.image-preview-bar')).toHaveCount(0);
    expect(await history(founder, created.channel_id)).toHaveLength(0);
    await founder.page.locator('.message-input input[type="file"]').setInputFiles(file);
    const caption = `${run}-image-caption`;
    await founder.page.getByPlaceholder('Add a caption (optional)...').fill(caption);
    const uploaded = founder.page.waitForResponse(response =>
      response.request().method() === 'POST' && new URL(response.url()).pathname.includes('/upload'));
    await founder.page.getByRole('button', { name: 'Send', exact: true }).click();
    expect((await uploaded).ok()).toBe(true);
    const remote = member.page.locator('.message', { hasText: caption });
    await expect(remote.locator('.message-inline-image')).toBeVisible();
    await expect.poll(() => remote.locator('.message-inline-image').evaluate(image =>
      (image as HTMLImageElement).naturalWidth)).toBe(4);
    const persisted = (await history(member, created.channel_id)).find(item => item.content.includes(caption));
    expect(persisted?.content).toContain('/uploads/');
    await remote.getByRole('button', { name: 'View image full size', exact: true }).click();
    await expect(member.page.getByRole('dialog')).toBeVisible();
    await member.page.keyboard.press('Escape');
    await expect(member.page.getByRole('dialog')).toHaveCount(0);
    await reload(member);
    await select(member, name);
    await expect.poll(() => member.page.locator('.message-inline-image').evaluate(image =>
      (image as HTMLImageElement).naturalWidth)).toBe(4);
    await evidence('remote-upload-decodes-after-reload', member);
    await removeChannel(name, created.channel_id);
  });

  test('moderator grant enables member channel creation and revocation removes access', async () => {
    const memberIdentity = (await publicIdentity(member.page))[0];
    await founder.page.locator('.admin-menu-btn').click();
    await founder.page.getByRole('button', { name: 'Member Management', exact: true }).click();
    const memberRow = founder.page.locator('.member-row').filter({
      has: founder.page.locator(`.member-pseudonym[title="${memberIdentity.pseudonymId}"]`),
    });
    await expect(memberRow.getByLabel('Moderator', { exact: true })).not.toBeChecked();
    // This controlled checkbox reflects the committed server response. Its
    // native checked state can revert while the request is pending, so click
    // once and wait for the persisted state instead of check()'s immediate test.
    await memberRow.getByLabel('Moderator', { exact: true }).click();
    await expect(memberRow.getByLabel('Moderator', { exact: true })).toBeChecked();
    await reload(member);
    await expect(member.page.locator('.admin-menu-btn')).toBeVisible();
    await expect(member.page.locator('.create-channel-btn')).toBeVisible();
    const name = `${run}-delegated`;
    const created = await create(name, 'Text', false, member);
    expect((await channel(founder, name)).channel_id).toBe(created.channel_id);
    await memberRow.getByLabel('Moderator', { exact: true }).click();
    await expect(memberRow.getByLabel('Moderator', { exact: true })).not.toBeChecked();
    await reload(member);
    await expect(member.page.locator('.admin-menu-btn')).toHaveCount(0);
    await expect(member.page.locator('.create-channel-btn')).toHaveCount(0);
    const denied = await member.context.request.get(new URL('/api/admin/policy', baseURL).href, { headers: member.headers });
    expect(denied.status()).toBe(403);
    await founder.page.getByRole('button', { name: 'Chat', exact: true }).click();
    await removeChannel(name, created.channel_id);
    await evidence('moderator-access-revoked', member);
  });

  test('persona create, edit, assignment and delete persist locally', async () => {
    const page = founder.page;
    const name = `${run}-persona`.slice(0, 30);
    const edited = `${name.slice(0, 26)}-edit`;
    await page.getByRole('button', { name: 'Identity', exact: true }).click();
    let dialog = page.getByRole('dialog');
    await dialog.getByRole('button', { name: 'New Persona', exact: true }).click();
    await dialog.getByPlaceholder('e.g. seismicgear').fill(name);
    await dialog.getByPlaceholder('Optional status or bio').fill('Live acceptance persona');
    await dialog.locator('.persona-form .color-swatch').nth(2).click();
    await dialog.getByRole('button', { name: 'Create Persona', exact: true }).click();
    let persona = dialog.locator('.persona-item', { hasText: name });
    await expect(persona).toBeVisible();
    const activate = persona.getByRole('button', { name: 'Use', exact: true });
    if (await activate.isVisible()) await activate.click();
    await expect(persona).toHaveClass(/active/);
    await persona.getByRole('button', { name: 'Edit', exact: true }).click();
    await dialog.getByPlaceholder('e.g. seismicgear').fill(edited);
    await dialog.getByRole('button', { name: 'Save Changes', exact: true }).click();
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await reload(founder);
    await page.getByRole('button', { name: 'Identity', exact: true }).click();
    dialog = page.getByRole('dialog');
    persona = dialog.locator('.persona-item', { hasText: edited });
    await expect(persona).toHaveClass(/active/);
    await expect(persona).toContainText('Live acceptance persona');
    await persona.getByRole('button', { name: 'Del', exact: true }).click();
    await expect(persona).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await reload(founder);
    await page.getByRole('button', { name: 'Identity', exact: true }).click();
    await expect(page.locator('.persona-item', { hasText: edited })).toHaveCount(0);
    await page.getByRole('dialog').getByRole('button', { name: 'Close', exact: true }).click();
    await evidence('persona-deletion-persists');
  });
});
