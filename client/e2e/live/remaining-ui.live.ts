import { chromium, expect, test, type BrowserContext, type Locator, type Page } from '@playwright/test';
import { spawn } from 'node:child_process';
import { mkdir, mkdtemp, readdir, rm } from 'node:fs/promises';
import { basename, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { admin, baseURL, newActor, read, select, send } from './operator-helpers';
import type { Channel } from '../../src/types';

// OS file sharing, actual external HTTP, and naturally occurring layout only.
// No application stores, IndexedDB functions or API outcomes are replaced.
async function removeStartupProfile(root: string, profile: string) {
  const checked = resolve(profile);
  if (!checked.startsWith(`${resolve(root)}${sep}`) || !basename(checked).startsWith('annex-startup-lock-')) throw new Error('Refusing unexpected disposable profile cleanup');
  await rm(checked, { recursive: true, force: true });
}
async function refusedStartup(action: 'Retry startup' | 'Clear local state') {
  expect(process.platform, 'This acceptance fault uses an actual Windows file-sharing refusal').toBe('win32');
  const root = fileURLToPath(new URL('../../../../startup-storage-profiles/', import.meta.url));
  await mkdir(root, { recursive: true }); const profile = await mkdtemp(join(root, 'annex-startup-lock-'));
  let context: BrowserContext | undefined; let release: (() => Promise<void>) | undefined;
  const requests: string[] = [];
  let activePage: Page | undefined; let initialErrorDetails: string | null = null;
  try {
    context = await chromium.launchPersistentContext(profile, { baseURL });
    let page = context.pages()[0] ?? await context.newPage(); await page.goto('/');
    await expect(page.getByRole('button', { name: 'Create New Identity', exact: true })).toBeVisible();
    await context.close(); context = undefined;
    const directory = join(profile, 'Default', 'IndexedDB');
    const databases = (await readdir(directory, { withFileTypes: true })).filter(entry => entry.isDirectory() && entry.name.endsWith('.indexeddb.leveldb'));
    expect(databases, 'Actual normal startup must create its own origin database before the fault').toHaveLength(1);
    const lock = join(directory, databases[0].name, 'LOCK');
    const child = spawn(process.env.ANNEX_TEST_POWERSHELL ?? 'pwsh.exe', ['-NoProfile', '-ExecutionPolicy', 'Bypass', '-File',
      fileURLToPath(new URL('../../../scripts/live/hold-indexeddb-lock.ps1', import.meta.url)), '-Path', lock, '-ProfileRoot', profile],
    { stdio: 'pipe', windowsHide: true });
    child.stderr.resume();
    const exit = new Promise<void>((resolveExit, reject) => {
      child.once('exit', code => code === 0 ? resolveExit() : reject(new Error(`Actual OS lock helper exited ${code}`)));
      child.once('error', reject);
    });
    // Observe rejection immediately; cleanup still awaits and reports it.
    void exit.catch(() => {});
    release = async () => { child.stdin.end('release\n'); await exit; };
    await new Promise<void>((resolveReady, reject) => {
      const timer = setTimeout(() => reject(new Error('Actual exclusive IndexedDB LOCK was not acquired')), 10_000);
      let output = '';
      child.stdout.on('data', bytes => { output += String(bytes); if (output.includes('LOCKED')) { clearTimeout(timer); resolveReady(); } });
      child.once('exit', code => { clearTimeout(timer); reject(new Error(`IndexedDB lock helper stopped before acceptance (${code})`)); });
      child.once('error', error => { clearTimeout(timer); reject(error); });
    });
    context = await chromium.launchPersistentContext(profile, { baseURL });
    page = context.pages()[0] ?? await context.newPage(); activePage = page;
    page.on('request', request => { if (request.method() !== 'GET') requests.push(new URL(request.url()).pathname); });
    await page.goto('/');
    const failure = page.getByRole('alert').filter({ hasText: 'Startup failed:' });
    await expect(failure, 'The held native LOCK must actually refuse application storage; absent fault is not a product failure').toBeVisible();
    await page.locator('.error-details > summary').click(); await expect(page.locator('.error-details[open] pre')).not.toBeEmpty();
    const detail = await page.locator('.error-details[open] pre').textContent(); initialErrorDetails = detail;
    await release(); release = undefined;
    await page.getByRole('button', { name: action, exact: true }).click();
    await expect(page.getByRole('button', { name: 'Create New Identity', exact: true })).toBeVisible();
    await expect(failure).toHaveCount(0); expect(requests).toEqual([]);
    await test.info().attach('actual-startup-storage-recovery', { contentType: 'application/json', body: Buffer.from(JSON.stringify({
      action, fault: 'Actual Windows exclusive file-sharing refusal on the existing empty IndexedDB LevelDB LOCK',
      nativeErrorDetails: detail, recoveredThroughControl: true, identitiesCreated: 0,
    })) });
  } finally {
    // This profile has no identity or keys. Preserve only its actual startup
    // controls and native error text even when the recovery assertion fails.
    // Diagnostic collection must never replace the original test failure.
    if (activePage && !activePage.isClosed()) {
      try {
        const observed = {
          action, initialErrorDetails,
          alerts: await activePage.getByRole('alert').allTextContents(),
          details: await activePage.locator('.error-details pre').allTextContents(),
          buttons: await activePage.getByRole('button').allTextContents(),
          nonReadRequestPaths: requests,
        };
        await test.info().attach('private-startup-recovery-diagnostics', { contentType: 'application/json',
          body: Buffer.from(JSON.stringify(observed)) });
      } catch { /* Preserve the original browser/assertion error. */ }
    }
    try { await release?.(); } finally {
      try { await context?.close(); } finally { await removeStartupProfile(root, profile); }
    }
  }
}

test('actual browser storage refusal exposes Details and Retry startup recovers', async () => {
  await refusedStartup('Retry startup');
});
test('actual browser storage refusal can clear an empty disposable profile and recover', async () => {
  await refusedStartup('Clear local state');
});

interface TipBounds { left: number; right: number; width: number; viewport: number; uncorrectedLeft: number; shift: number }
async function tipBounds(tip: Locator): Promise<TipBounds> {
  return tip.evaluate(element => {
    const popup = element.querySelector<HTMLElement>('[role="tooltip"]')!;
    const icon = element.getBoundingClientRect(); const rect = popup.getBoundingClientRect();
    return { left: rect.left, right: rect.right, width: rect.width, viewport: window.innerWidth,
      uncorrectedLeft: icon.left + icon.width / 2 - rect.width / 2,
      shift: Number.parseFloat(popup.style.getPropertyValue('--tip-shift')) || 0 };
  });
}

test('real public previews, unavailable links, and naturally overflowing tooltips preserve their contracts', async ({ browser }) => {
  const founder = await newActor(browser); const member = await newActor(browser);
  const publicURL = process.env.ANNEX_PREVIEW_URL ?? 'https://ogp.me/';
  const receipts: Record<string, unknown> = {};
  const network: Record<string, unknown>[] = []; const inspections: Promise<void>[] = [];
  for (const [index, actor] of [founder, member].entries()) actor.page.on('response', response => {
    const url = new URL(response.url()); if (!url.pathname.startsWith('/api/link-preview')) return;
    const inspect = (async () => {
      const requestHeaders = response.request().headers();
      const entry: Record<string, unknown> = { actor: index, path: url.pathname, target: url.searchParams.get('url'),
        status: response.status(), bearerPresent: Boolean(requestHeaders.authorization), proofPresent: Boolean(requestHeaders['x-annex-zk-proof']) };
      network.push(entry);
      try { const bytes = await response.body(); entry.responseBytes = bytes.length;
        entry.responseSha256 = createHash('sha256').update(bytes).digest('hex');
        if (url.pathname === '/api/link-preview' && bytes.length < 16_384) entry.actualMetadataResponse = bytes.toString('utf8');
      } catch { entry.bodyUnavailable = true; }
    })(); inspections.push(inspect);
  });
  try {
    await expect(founder.page.locator('.admin-menu-btn'), 'Use a fresh dedicated database').toBeVisible();
    const name = `remaining-ui-${Date.now()}`;
    const created = await founder.context.request.post(`${baseURL}/api/channels`, { headers: founder.headers,
      data: { channel_id: name, name, channel_type: 'Text', federation_scope: 'Local' } });
    expect(created.status()).toBe(200);
    const channel = (await read<Channel[]>(founder, '/api/channels')).find(item => item.channel_id === name)!;
    await select(founder, name); await select(member, name);

    await test.step('Actual public HTML and image reach both peers solely through the preview proxy', async () => {
      const original = await founder.context.request.get(publicURL, { timeout: 30_000 });
      expect(original.ok(), 'The configured public site must actually serve its page to establish this acceptance condition').toBe(true);
      const html = await original.text();
      receipts.externalPage = { url: publicURL, status: original.status(), bytes: Buffer.byteLength(html),
        sha256: createHash('sha256').update(html).digest('hex') };
      expect(Buffer.byteLength(html), 'The actual external HTML must fit the shipped 512 KiB proxy bound').toBeLessThanOrEqual(512 * 1024);
      const independent = await founder.page.evaluate(source => {
        const document = new DOMParser().parseFromString(source, 'text/html');
        return { title: document.querySelector('meta[property="og:title"]')?.getAttribute('content')?.trim()
          || document.querySelector('title')?.textContent?.trim(),
        image: document.querySelector('meta[property="og:image"]')?.getAttribute('content')?.trim() };
      }, html);
      expect(independent.title).toBeTruthy(); expect(independent.image, 'Use a public target with a real image preview').toBeTruthy();
      const browserHTTP: string[] = [];
      for (const actor of [founder, member]) actor.page.on('request', request => {
        const url = new URL(request.url()); if (url.protocol === 'http:' || url.protocol === 'https:') browserHTTP.push(url.href);
      });
      const persisted = await send(founder, name, publicURL);
      for (const actor of [founder, member]) {
        const card = actor.page.locator('.link-preview-card').filter({ has: actor.page.locator('.link-preview-title', { hasText: independent.title! }) });
        await expect(card).toHaveCount(1); await expect(card).toHaveAttribute('href', publicURL);
        await expect(card.locator('.link-preview-title')).toHaveText(independent.title!);
        await expect(card).toHaveAttribute('rel', 'noopener noreferrer');
        const image = card.locator('.link-preview-image img'); await image.scrollIntoViewIfNeeded();
        await expect.poll(() => image.evaluate((element: HTMLImageElement) => element.complete && element.naturalWidth > 0)).toBe(true);
        const src = await image.getAttribute('src'); expect(src).toBeTruthy();
        const proxied = new URL(src!, baseURL); expect(proxied.origin).toBe(new URL(baseURL).origin);
        expect(proxied.pathname).toBe('/api/link-preview/image');
      }
      expect(browserHTTP.filter(url => new URL(url).origin !== new URL(baseURL).origin), 'Rendering a card must not contact the third-party site from either browser').toEqual([]);
      const popupPromise = member.page.waitForEvent('popup');
      await member.page.locator('.link-preview-card').click();
      const popup = await popupPromise; await popup.waitForLoadState('domcontentloaded');
      expect(new URL(popup.url()).origin).toBe(new URL(publicURL).origin); await popup.close();
      receipts.publicPreview = { messageId: persisted.message_id, url: publicURL,
        independentlyFetchedHtmlSha256: createHash('sha256').update(html).digest('hex'),
        displayedTitle: independent.title, browserThirdPartyRequestsBeforeClick: 0 };
    });

    await test.step('Real DNS failure and private-address refusal retain a safe clickable fallback', async () => {
      const failures: Array<{ url: string; status: number; messageId: string }> = [];
      for (const url of ['https://annex-acceptance-unreachable.invalid/article', 'http://127.0.0.1:1/private-preview']) {
        const response = member.page.waitForResponse(item => new URL(item.url()).pathname === '/api/link-preview'
          && new URL(item.url()).searchParams.get('url') === url);
        const message = await send(founder, name, url); const actual = await response;
        expect(actual.ok()).toBe(false);
        if (url.includes('127.0.0.1')) expect(actual.status()).toBe(403);
        for (const actor of [founder, member]) {
          const fallback = actor.page.locator('.link-preview-minimal').filter({ hasText: url });
          await expect(fallback).toBeVisible(); await expect(fallback).toHaveAttribute('href', url);
          await expect(fallback).toHaveAttribute('rel', 'noopener noreferrer');
        }
        failures.push({ url, status: actual.status(), messageId: message.message_id });
      }
      receipts.unavailablePreviews = failures;
    });

    await test.step('Naturally overflowing policy tips stay within the viewport on keyboard focus and hover', async () => {
      await founder.page.setViewportSize({ width: 360, height: 740 });
      await admin(founder, 'Server Settings');
      const tips = founder.page.locator('.info-tip'); let corrected: TipBounds | undefined;
      for (let index = 0; index < await tips.count(); index++) {
        const tip = tips.nth(index); if (!await tip.isVisible()) continue;
        await tip.scrollIntoViewIfNeeded(); await tip.focus(); await expect(tip.getByRole('tooltip')).toBeVisible();
        const bounds = await tipBounds(tip);
        if (bounds.uncorrectedLeft < 8 || bounds.uncorrectedLeft + bounds.width > bounds.viewport - 8) {
          await expect.poll(async () => {
            const actual = await tipBounds(tip); return actual.left >= 7 && actual.right <= actual.viewport - 7 && actual.shift !== 0;
          }, { message: 'Actual overflow must be corrected without moving or restyling the source element in the test' }).toBe(true);
          corrected = await tipBounds(tip);
          await founder.page.keyboard.press('Tab'); await founder.page.mouse.move(0, 0);
          await expect(tip.getByRole('tooltip')).toHaveCount(0);
          await tip.hover(); await expect(tip.getByRole('tooltip')).toBeVisible();
          const hovered = await tipBounds(tip); expect(hovered.left).toBeGreaterThanOrEqual(7);
          expect(hovered.right).toBeLessThanOrEqual(hovered.viewport - 7);
          await founder.page.mouse.move(0, 0); await expect(tip.getByRole('tooltip')).toHaveCount(0);
          break;
        }
        await founder.page.keyboard.press('Tab'); await founder.page.mouse.move(0, 0);
      }
      expect(corrected, 'At least one actual policy tooltip must require the edge correction; merely fitting an ordinary tooltip is insufficient').toBeDefined();
      receipts.edgeTooltip = corrected;
    });
    expect(founder.errors).toEqual([]); expect(member.errors).toEqual([]);
    await test.info().attach('remaining-ui-real-evidence', { contentType: 'application/json',
      body: Buffer.from(JSON.stringify({ complete: true, channelId: channel.channel_id, receipts })) });
  } finally {
    await Promise.allSettled(inspections);
    const dom = await Promise.all([founder, member].map(async actor => ({
      cards: await actor.page.locator('.link-preview-card').count(),
      fallback: await actor.page.locator('.link-preview-minimal').count(),
      loading: await actor.page.locator('.link-preview.loading').count(),
      titles: await actor.page.locator('.link-preview-title').allTextContents(),
    })));
    await test.info().attach('actual-preview-network-and-dom', { contentType: 'application/json',
      body: Buffer.from(JSON.stringify({ receipts, network, dom })) });
    await member.context.close(); await founder.context.close();
  }
});
