// Focused UX tour: capture the states the happy-path harness skips —
// loading/proof generation, errors, empty states, voice-not-configured,
// federation/events tabs, and dialogs. Screenshots → screenshots/ux-*.png
import { existsSync, readdirSync, mkdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import puppeteer from 'puppeteer-core';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = join(__dirname, 'screenshots');
mkdirSync(SHOT_DIR, { recursive: true });
const URL = process.env.URL || 'http://127.0.0.1:3000';

function resolveChrome() {
  const pwRoot = process.env.PLAYWRIGHT_BROWSERS_PATH || '/opt/pw-browsers';
  if (existsSync(pwRoot))
    for (const e of readdirSync(pwRoot))
      if (e.startsWith('chromium-')) {
        const c = join(pwRoot, e, 'chrome-linux', 'chrome');
        if (existsSync(c)) return c;
      }
  return null;
}
const log = (m) => console.log(`[ux] ${m}`);
let n = 0;
async function shot(page, name) {
  const f = join(SHOT_DIR, `ux-${String(++n).padStart(2, '0')}-${name}.png`);
  await page.screenshot({ path: f, fullPage: false });
  log(`shot → ${f}`);
}
async function clickText(page, text, ms = 20000) {
  const h = await page
    .waitForFunction(
      (t) => [...document.querySelectorAll('button,[role="button"],a,.tab,[class*="tab"]')].find(
        (el) => (el.textContent || '').trim() === t || (el.textContent || '').trim().includes(t)),
      { timeout: ms, polling: 150 }, text)
    .catch(() => null);
  if (!h) { log(`(no clickable "${text}")`); return false; }
  const el = h.asElement(); if (!el) return false;
  await el.click(); return true;
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const errors = [];
(async () => {
  const exe = resolveChrome();
  const browser = await puppeteer.launch({ executablePath: exe, headless: true, args: ['--no-sandbox', '--disable-dev-shm-usage'] });
  const page = await browser.newPage();
  await page.setViewport({ width: 1280, height: 820 });
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', (e) => errors.push('PAGEERROR: ' + e.message));

  log(`goto ${URL}`);
  await page.goto(URL, { waitUntil: 'networkidle2', timeout: 60000 });
  await sleep(500);
  await shot(page, 'identity-setup');

  // Create identity
  await clickText(page, 'Create New Identity');
  await sleep(400);
  await shot(page, 'after-create-click');
  // Server selection step
  await page.waitForFunction(
    () => [...document.querySelectorAll('button')].some((b) => /continue/i.test(b.textContent || '')),
    { timeout: 30000 }).catch(() => log('no Continue button'));
  await shot(page, 'server-select');

  // Click continue and immediately capture the proof-generation loading state
  await clickText(page, 'Continue');
  await sleep(250);
  await shot(page, 'proof-loading-250ms');
  await sleep(1200);
  await shot(page, 'proof-loading-1450ms');

  // Wait for main UI
  await page.waitForFunction(
    () => /Select a channel|CHANNELS|General/i.test(document.body.innerText),
    { timeout: 120000 }).catch(() => log('main UI did not appear'));
  await sleep(500);
  await shot(page, 'main-empty');

  // Try sending a message WITHOUT joining a channel (error/empty path)
  const ta = await page.$('textarea, input[type="text"][placeholder*="essage"], input[placeholder*="essage"]');
  if (ta) { await ta.type('hello before joining'); await sleep(200); await shot(page, 'type-before-join'); }

  // Federation tab
  if (await clickText(page, 'Federation')) { await sleep(700); await shot(page, 'federation-tab'); }
  // Events tab
  if (await clickText(page, 'Events')) { await sleep(700); await shot(page, 'events-tab'); }
  // Back to Chat
  await clickText(page, 'Chat'); await sleep(400);

  // Join General then open voice if present
  await clickText(page, 'General'); await sleep(500);
  await shot(page, 'channel-selected');
  // Look for a voice/call control
  for (const t of ['Voice', 'Join Voice', 'Call', '🔊']) {
    if (await clickText(page, t, 2500)) { await sleep(800); await shot(page, 'voice-attempt'); break; }
  }

  // Open footer dialogs
  for (const t of ['Recovery', 'Export', 'Identity', 'Link']) {
    if (await clickText(page, t, 2500)) { await sleep(600); await shot(page, `dialog-${t.toLowerCase()}`);
      // close via Escape
      await page.keyboard.press('Escape'); await sleep(300); }
  }
  // Settings gear
  const gear = await page.$('[aria-label*="ettings"], [title*="ettings"], button[class*="gear"], button[class*="settings"]');
  if (gear) { await gear.click(); await sleep(500); await shot(page, 'settings'); }

  log('console errors captured: ' + errors.length);
  for (const e of errors.slice(0, 25)) log('  ERR ' + e.slice(0, 200));
  await browser.close();
  log('done');
})().catch((e) => { console.error('[ux] FAIL', e); process.exit(1); });
