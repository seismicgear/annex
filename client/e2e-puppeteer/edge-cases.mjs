#!/usr/bin/env node
//
// edge-cases.mjs — "users doing weird things." Hammers the chat UI with
// hostile/sloppy interactions and asserts the app never crashes, never throws
// an uncaught error, and stays responsive (main layout present at the end).
//
// Cases: empty send (button disabled), very long message, rapid-fire flooding,
// double-click join, rapid channel switching, double-delete, mid-session
// reload. Any uncaught page error or a missing main layout fails the run.
//
// Usage: node client/e2e-puppeteer/edge-cases.mjs [--url http://127.0.0.1:3000]

import { existsSync, mkdirSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execSync } from 'node:child_process';
import puppeteer from 'puppeteer-core';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = join(__dirname, 'screenshots-edge');
let URL_ = 'http://127.0.0.1:3000';
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) if (argv[i] === '--url') URL_ = argv[++i];

const log = (m) => console.log(`[edge] ${m}`);
function fail(m, e) {
  console.error(`[edge] FAIL: ${m}`);
  if (e) console.error(e.stack ?? e.message ?? String(e));
  process.exit(1);
}
function resolveChrome() {
  const env = process.env.PUPPETEER_EXECUTABLE_PATH;
  if (env && existsSync(env)) return env;
  const pwRoot = process.env.PLAYWRIGHT_BROWSERS_PATH || '/opt/pw-browsers';
  if (existsSync(pwRoot)) {
    for (const e of readdirSync(pwRoot)) {
      if (e.startsWith('chromium-')) {
        const c = join(pwRoot, e, 'chrome-linux', 'chrome');
        if (existsSync(c)) return c;
      }
    }
  }
  for (const b of ['google-chrome-stable', 'google-chrome', 'chromium', 'chromium-browser']) {
    try {
      const p = execSync(`command -v ${b}`, { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim();
      if (p && existsSync(p)) return p;
    } catch { /* keep looking */ }
  }
  return null;
}
async function clickByText(page, text, t = 30_000) {
  const h = await page.waitForFunction(
    (l) => [...document.querySelectorAll('button,[role="button"]')].find((el) => (el.textContent || '').trim() === l) || null,
    { timeout: t, polling: 200 }, text,
  ).catch(() => null);
  const el = h && h.asElement();
  if (!el) return false;
  await el.click();
  return true;
}
const waitForBtn = (page, text, t = 90_000) =>
  page.waitForFunction((l) => [...document.querySelectorAll('button,[role="button"]')].some((el) => (el.textContent || '').trim() === l), { timeout: t, polling: 250 }, text).then(() => true).catch(() => false);
const waitForSel = (page, sel, t = 20_000) => page.waitForSelector(sel, { timeout: t }).then(() => true).catch(() => false);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function startup(page) {
  await page.goto(URL_, { waitUntil: 'domcontentloaded', timeout: 60_000 });
  if (!(await waitForBtn(page, 'Create New Identity', 30_000))) fail('identity setup did not render');
  await clickByText(page, 'Create New Identity', 15_000);
  if (await waitForBtn(page, 'Continue', 30_000)) await clickByText(page, 'Continue', 15_000);
  if (!(await waitForBtn(page, 'Chat', 120_000))) fail('main UI never appeared');
  await sleep(1200);
}
async function selectGeneralAndJoin(page) {
  await page.evaluate(() => {
    const g = [...document.querySelectorAll('.channel-item')].find((r) => (r.textContent || '').includes('General'));
    g?.querySelector('.join-btn')?.click();
  });
  await sleep(800);
  await page.evaluate(() => {
    const g = [...document.querySelectorAll('.channel-item')].find((r) => (r.textContent || '').includes('General'));
    g?.querySelector('.channel-select')?.click();
  });
  await waitForSel(page, 'textarea[placeholder="Type a message..."]', 20_000);
}

async function main() {
  const exe = resolveChrome();
  if (!exe) fail('no Chrome/Chromium found');
  try {
    const res = await fetch(`${URL_}/health`);
    if (!res.ok || (await res.json()).status !== 'ok') fail('server /health not ok');
  } catch (e) { fail(`cannot reach ${URL_}/health`, e); }
  mkdirSync(SHOT_DIR, { recursive: true });

  const browser = await puppeteer.launch({
    executablePath: exe, headless: true,
    args: ['--no-sandbox', '--disable-setuid-sandbox', '--disable-dev-shm-usage', '--use-gl=swiftshader', '--window-size=1280,900'],
    defaultViewport: { width: 1280, height: 900 },
  });
  const pageErrors = [];
  try {
    const page = await browser.newPage();
    page.on('pageerror', (e) => { pageErrors.push(e.message); log(`pageerror: ${e.message}`); });

    await startup(page);
    await selectGeneralAndJoin(page);
    const composer = 'textarea[placeholder="Type a message..."]';

    // 1. Empty message → Send must be disabled (can't post nothing).
    await page.click(composer);
    const sendDisabledOnEmpty = await page.evaluate(() => {
      const btns = [...document.querySelectorAll('button')];
      const send = btns.find((b) => (b.textContent || '').trim() === 'Send');
      return send ? send.disabled : null;
    });
    if (sendDisabledOnEmpty !== true) fail(`Send should be disabled for empty input (got disabled=${sendDisabledOnEmpty})`);
    log('PASS: empty message cannot be sent (Send disabled)');

    // 2. Very long message (8000 chars) — must be handled without crashing.
    const long = 'L'.repeat(8000);
    await page.type(composer, long, { delay: 0 });
    await clickByText(page, 'Send', 5_000);
    await sleep(1500);
    log('PASS: 8000-char message handled without crash');

    // 3. Rapid-fire flood: 10 quick messages.
    for (let i = 0; i < 10; i++) {
      await page.type(composer, `flood-${i}`);
      await clickByText(page, 'Send', 3_000).catch(() => {});
      await sleep(120);
    }
    await sleep(1500);
    log('PASS: 10 rapid-fire messages handled');

    // 4. Double-click join on General (idempotent membership).
    await page.evaluate(() => {
      const g = [...document.querySelectorAll('.channel-item')].find((r) => (r.textContent || '').includes('General'));
      const j = g?.querySelector('.join-btn');
      if (j) { j.click(); j.click(); }
    });
    await sleep(800);
    log('PASS: double-join handled');

    // 5. Rapid tab/channel switching.
    for (let i = 0; i < 6; i++) {
      await clickByText(page, i % 2 === 0 ? 'Federation' : 'Events', 5_000).catch(() => {});
      await sleep(150);
    }
    await clickByText(page, 'Chat', 5_000);
    await sleep(500);
    if (!(await waitForSel(page, '.app-layout', 10_000))) fail('app layout missing after rapid tab switching');
    log('PASS: rapid tab switching kept the app alive');

    // 6. Double-delete: send a msg, delete it twice quickly.
    await selectGeneralAndJoin(page);
    const delText = `del-${Date.now()}`;
    await page.type(composer, delText);
    await clickByText(page, 'Send', 5_000);
    await waitForSel(page, `.message`, 10_000);
    await page.evaluate((t) => {
      const m = [...document.querySelectorAll('.message')].find((el) => (el.textContent || '').includes(t));
      const d = m?.querySelector('.delete-btn');
      if (d) { d.click(); d.click(); d.click(); } // confirm + extra clicks
    }, delText);
    await sleep(1500);
    log('PASS: rapid double/triple-delete handled');

    // 7. Mid-session reload — app must restore to the main UI.
    await page.reload({ waitUntil: 'domcontentloaded', timeout: 60_000 });
    if (await waitForBtn(page, 'Continue', 20_000)) await clickByText(page, 'Continue', 15_000);
    if (!(await waitForBtn(page, 'Chat', 60_000))) fail('app did not restore after mid-session reload');
    await waitForSel(page, '.app-layout', 15_000);
    await page.screenshot({ path: join(SHOT_DIR, '01-after-edge-cases.png'), fullPage: true });
    log('PASS: app restored after mid-session reload');

    // Final: no uncaught page errors throughout the whole gauntlet.
    if (pageErrors.length > 0) {
      fail(`uncaught page errors during edge-case gauntlet: ${pageErrors.join(' | ')}`);
    }
    log('OK — edge-case gauntlet passed with zero uncaught page errors');
  } finally {
    await browser.close();
  }
}

main().then(() => process.exit(0)).catch((err) => fail('unexpected error', err));
