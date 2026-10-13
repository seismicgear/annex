import { expect, type Browser, type BrowserContext, type Page } from '@playwright/test';
import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import { createInterface } from 'node:readline';
import { completeStartup } from '../helpers';
import type { FederationPeer } from '../../src/types';

export interface Actor { context: BrowserContext; page: Page; url: string; credentials: Map<string, Record<string, string>>; errors: string[] }
export interface RecordFile { url: string; binary_sha256: string }
interface Receipt { id: string; type: string; action: string; error?: string; channelId?: string; messageId?: string;
  matching?: Array<{ messageId: string; content: string; sender: string }>; [key: string]: unknown }
export class Companion {
  child: ChildProcessWithoutNullStreams;
  private pending = new Map<string, { resolve: (value: Receipt) => void; reject: (error: Error) => void; timer: NodeJS.Timeout }>();
  constructor(files: string[]) {
    this.child = spawn(process.execPath, [fileURLToPath(new URL('../../../scripts/live/federation-ui.mjs', import.meta.url)), ...files],
      { stdio: 'pipe', windowsHide: true });
    createInterface({ input: this.child.stdout }).on('line', line => {
      const value = JSON.parse(line) as Receipt; const item = this.pending.get(value.id); if (!item) return;
      clearTimeout(item.timer); this.pending.delete(value.id);
      if (value.type === 'error') item.reject(new Error(`${value.action}: ${value.error}`)); else item.resolve(value);
    });
    // No credentials are accepted on stderr; avoid incorporating arbitrary
    // child output into evidence. An exit is itself an explicit failure.
    this.child.stderr.resume();
    this.child.on('exit', code => { for (const item of this.pending.values()) {
      clearTimeout(item.timer); item.reject(new Error(`Live federation companion exited (${code})`));
    } this.pending.clear(); });
  }
  async command(action: string, body: Record<string, unknown> = {}): Promise<Receipt> {
    const id = crypto.randomUUID();
    const result = new Promise<Receipt>((resolve, reject) => {
      const timer = setTimeout(() => { this.pending.delete(id); reject(new Error(`Actual federation ${action} timed out`)); }, 150_000);
      this.pending.set(id, { resolve, reject, timer });
    });
    this.child.stdin.write(`${JSON.stringify({ id, action, ...body })}\n`); return result;
  }
  async close() { try { if (this.child.exitCode === null) await this.command('close'); } finally { this.child.stdin.end(); } }
}
export async function actor(browser: Browser, url: string): Promise<Actor> {
  const context = await browser.newContext({ baseURL: url }); const page = await context.newPage();
  const who: Actor = { context, page, url, credentials: new Map(), errors: [] };
  page.on('pageerror', error => who.errors.push(error.message));
  page.on('request', request => {
    const headers = request.headers(); if (headers.authorization && headers['x-annex-zk-proof']) {
      who.credentials.set(new URL(request.url()).origin, Object.fromEntries(Object.entries(headers)
        .filter(([name]) => name === 'authorization' || name.startsWith('x-annex-'))));
    }
  });
  await completeStartup(page); return who;
}
export async function auth(who: Actor) {
  await expect.poll(() => who.credentials.has(new URL(who.url).origin)).toBe(true);
  return who.credentials.get(new URL(who.url).origin)!;
}
export async function get<T>(who: Actor, path: string): Promise<T> {
  const response = await who.context.request.get(new URL(path, who.url).href, { headers: await auth(who) });
  expect(response.status(), `Actual authenticated ${path}`).toBe(200); return response.json() as Promise<T>;
}
export async function delivery(who: Actor) {
  await who.page.locator('.admin-menu-btn').click();
  await who.page.getByRole('button', { name: 'Federation Delivery', exact: true }).click();
  await expect(who.page.getByRole('heading', { name: 'Federation Delivery', exact: true })).toBeVisible();
}
export async function explore(who: Actor, peer: FederationPeer) {
  await who.page.getByRole('button', { name: 'Federation', exact: true }).click();
  const entry = who.page.locator('.peer-item').filter({ has: who.page.locator('.peer-url', { hasText: peer.base_url }) });
  await entry.getByRole('button', { name: 'Explore', exact: true }).click();
  return who.page.getByRole('dialog', { name: 'Upstream Federation', exact: true });
}
