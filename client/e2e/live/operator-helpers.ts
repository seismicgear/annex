import { expect, type Browser, type BrowserContext, type Page, type Response } from '@playwright/test';
import { completeStartup } from '../helpers';
import type { Channel, Message, ServerPolicy } from '../../src/types';

export const baseURL = process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000';
export interface Actor { page: Page; context: BrowserContext; headers: Record<string, string>; errors: string[] }
export async function newActor(browser: Browser, register = true): Promise<Actor> {
  const context = await browser.newContext({ baseURL, permissions: ['clipboard-read', 'clipboard-write'] });
  const page = await context.newPage();
  const who: Actor = { page, context, headers: {}, errors: [] };
  page.on('pageerror', error => who.errors.push(error.message));
  page.on('request', request => {
    if (new URL(request.url()).origin !== new URL(baseURL).origin) return;
    const headers = request.headers();
    if (headers.authorization && headers['x-annex-zk-proof']) who.headers = Object.fromEntries(
      Object.entries(headers).filter(([key]) => key === 'authorization' || key.startsWith('x-annex-')));
  });
  if (register) await completeStartup(page);
  return who;
}
export async function read<T>(who: Actor, route: string): Promise<T> {
  await expect.poll(() => Boolean(who.headers.authorization)).toBe(true);
  const response = await who.context.request.get(new URL(route, baseURL).href, { headers: who.headers });
  expect(response.status(), `Authenticated GET ${route}`).toBe(200);
  return response.json() as Promise<T>;
}
export async function mutation(who: Actor, method: string, path: string, action: () => Promise<unknown>): Promise<Response> {
  const pending = who.page.waitForResponse(response => response.request().method() === method
    && new URL(response.url()).pathname === path);
  await action();
  const response = await pending;
  expect(response.ok(), `${method} ${path} must succeed on the actual server`).toBe(true);
  return response;
}
export async function publicIdentities(who: Actor) {
  return who.page.evaluate(async () => new Promise<Array<{ commitmentHex: string; pseudonymId: string | null }>>((resolve, reject) => {
    const opening = indexedDB.open('annex-identity');
    opening.onerror = () => reject(new Error('Identity database read failed'));
    opening.onsuccess = () => {
      const db = opening.result;
      if (!db.objectStoreNames.contains('identities')) { db.close(); resolve([]); return; }
      const request = db.transaction('identities', 'readonly').objectStore('identities').getAll();
      request.onerror = () => { db.close(); reject(new Error('Identity read failed')); };
      request.onsuccess = () => {
        resolve(request.result.map((item: { commitmentHex: string; pseudonymId: string | null }) => ({
          commitmentHex: item.commitmentHex, pseudonymId: item.pseudonymId,
        })));
        db.close();
      };
    };
  }));
}
export async function reload(who: Actor) {
  await who.page.reload();
  await expect(who.page.getByRole('button', { name: 'Chat', exact: true })).toBeVisible({ timeout: 150_000 });
}
export async function admin(who: Actor, section: string) {
  await who.page.locator('.admin-menu-btn').click();
  await who.page.getByRole('button', { name: section, exact: true }).click();
  await expect(who.page.getByRole('heading', { name: section, exact: true })).toBeVisible();
  await expect(who.page.locator('.admin-loading')).toHaveCount(0);
}
export async function savePolicy(who: Actor) {
  await mutation(who, 'PUT', '/api/admin/policy', () => who.page.getByRole('button', { name: 'Save Policy', exact: true }).click());
  await expect(who.page.locator('.success-message')).toContainText('Policy updated');
}
export const toggles = [
  ['federation_enabled', 'Federation Enabled'], ['voice_enabled', 'Voice Enabled'],
  ['usernames_enabled', 'Usernames Enabled'], ['images_enabled', 'Images Enabled'],
  ['videos_enabled', 'Videos Enabled'], ['files_enabled', 'Files Enabled'],
  ['allow_reflection_summaries', 'Allow Reflection Summaries'], ['allow_full_knowledge', 'Allow Full Knowledge Bundles'],
] as const;
export const numbers = [
  ['agent_min_alignment_score', 'Min Alignment Score'], ['max_members', 'Max Members'],
  ['default_retention_days', 'Retention (days)'], ['max_image_size_mb', 'Max Image Size (MB)'],
  ['max_video_size_mb', 'Max Video Size (MB)'], ['max_file_size_mb', 'Max File Size (MB)'],
] as const;
export async function fillPolicy(who: Actor, policy: ServerPolicy) {
  const page = who.page;
  await page.getByLabel(/^Access Mode/).selectOption(policy.access_mode);
  if (policy.access_mode === 'password') await page.getByPlaceholder('Enter server password...').fill(policy.access_password);
  // Enable fields before entering limits, then set each requested final toggle.
  for (const [, label] of toggles) await page.getByLabel(new RegExp(`^${label}`)).check();
  for (const [key, label] of numbers) {
    await page.locator('label').filter({ hasText: label }).locator('input[type="number"]').fill(String(policy[key]));
  }
  for (const [key, label] of [['registration_limit', 'Registration'], ['verification_limit', 'Verification'], ['default_limit', 'Default']] as const) {
    await page.getByLabel(label, { exact: true }).fill(String(policy.rate_limit[key]));
  }
  for (const [key, label] of toggles) await page.getByLabel(new RegExp(`^${label}`)).setChecked(policy[key]);
  for (const [key, label] of [['agent_required_capabilities', 'Add a required capability'], ['principles', 'Add a principle'], ['prohibited_actions', 'Add a prohibited action']] as const) {
    const input = page.getByLabel(label, { exact: true });
    const section = page.locator('.policy-section').filter({ has: input });
    while (await section.locator('.tag-item button').count()) await section.locator('.tag-item button').first().click();
    for (const text of policy[key]) { await input.fill(text); await input.press('Enter'); }
  }
}
export function row(who: Actor, name: string) {
  return who.page.locator('.channel-item').filter({ has: who.page.locator('.channel-name', { hasText: new RegExp(`^${name}$`) }) });
}
export async function select(who: Actor, name: string) {
  await who.page.getByRole('button', { name: 'Chat', exact: true }).click();
  const entry = row(who, name);
  await expect(entry).toBeVisible();
  if (await entry.locator('.join-btn').isVisible()) {
    const joining = who.page.waitForResponse(response => response.request().method() === 'POST'
      && new URL(response.url()).pathname.endsWith('/join'));
    await entry.locator('.join-btn').click();
    expect((await joining).ok()).toBe(true);
  }
  await entry.locator('.channel-select').click();
}
export async function send(who: Actor, name: string, text: string): Promise<Message> {
  await select(who, name);
  const channel = (await read<Channel[]>(who, '/api/channels')).find(item => item.name === name)!;
  await who.page.getByPlaceholder('Type a message...').fill(text);
  await who.page.getByRole('button', { name: 'Send', exact: true }).click();
  await expect(who.page.locator('.message', { has: who.page.locator('.message-content', { hasText: text }) }).locator('.reply-btn')).toBeVisible();
  let found: Message | undefined;
  await expect.poll(async () => {
    found = (await read<Message[]>(who, `/api/channels/${channel.channel_id}/messages?limit=100`)).find(item => item.content === text);
    return Boolean(found);
  }).toBe(true);
  return found!;
}
export async function startUnregistered(who: Actor) {
  await who.page.goto('/');
  await who.page.getByRole('button', { name: 'Create New Identity', exact: true }).click();
  const continueButton = who.page.getByRole('button', { name: 'Continue', exact: true });
  await expect(continueButton).toBeVisible();
  await continueButton.click();
}
