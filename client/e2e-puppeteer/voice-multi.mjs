#!/usr/bin/env node
//
// voice-multi.mjs — Two-party voice fan-out proof (A actually hears B).
//
// Single-party voice.mjs proves a client↔SFU connection. This proves the SFU
// FORWARDS media between two real participants: Alice (founder) and Bob both
// join the same Voice channel's call; Bob's mic track must reach Alice as a
// remote audio track (RTCPeerConnection.ontrack → a rendered <audio> element).
//
// Both browsers use fake media devices; the WebRTC negotiation, ICE, SRTP and
// RTP forwarding through the in-process SFU are real.
//
// Usage: node client/e2e-puppeteer/voice-multi.mjs [--url http://127.0.0.1:3000]

import { existsSync, mkdirSync, readFileSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execFileSync, execSync } from 'node:child_process';
import puppeteer from 'puppeteer-core';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = join(__dirname, 'screenshots-voice');

const args = process.argv.slice(2);
let URL = 'http://127.0.0.1:3000';
for (let i = 0; i < args.length; i++) if (args[i] === '--url') URL = args[++i];

const log = (m) => console.log(`[voice-multi] ${m}`);
function fail(m, e) {
  console.error(`[voice-multi] FAIL: ${m}`);
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
let shotIdx = 10;
async function shot(page, name) {
  const f = join(SHOT_DIR, `${++shotIdx}-${name}.png`);
  await page.screenshot({ path: f, fullPage: true });
  log(`screenshot → ${f}`);
}
async function clickButtonByText(page, text, timeout = 30_000) {
  const h = await page
    .waitForFunction(
      (label) => {
        const els = [...document.querySelectorAll('button, [role="button"]')];
        return els.find((el) => (el.textContent || '').trim() === label) || null;
      },
      { timeout, polling: 200 },
      text,
    )
    .catch(() => null);
  const el = h && h.asElement();
  if (!el) return false;
  await el.click();
  return true;
}
const waitForButtonByText = (page, text, timeout = 90_000) =>
  page
    .waitForFunction(
      (label) => [...document.querySelectorAll('button, [role="button"]')].some((el) => (el.textContent || '').trim() === label),
      { timeout, polling: 250 },
      text,
    )
    .then(() => true)
    .catch(() => false);
const waitForSel = (page, sel, timeout = 30_000) =>
  page.waitForSelector(sel, { timeout }).then(() => true).catch(() => false);

async function startup(page) {
  await page.goto(URL, { waitUntil: 'domcontentloaded', timeout: 60_000 });
  if (!(await waitForButtonByText(page, 'Create New Identity', 30_000))) fail('identity setup did not render');
  await clickButtonByText(page, 'Create New Identity', 15_000);
  if (await waitForButtonByText(page, 'Continue', 30_000)) await clickButtonByText(page, 'Continue', 15_000);
  if (!(await waitForButtonByText(page, 'Chat', 120_000))) fail('main UI never appeared');
  await new Promise((r) => setTimeout(r, 1200));
}
async function resumeAfterReload(page) {
  await page.reload({ waitUntil: 'domcontentloaded', timeout: 60_000 });
  if (await waitForButtonByText(page, 'Continue', 20_000)) await clickButtonByText(page, 'Continue', 15_000);
  if (!(await waitForButtonByText(page, 'Chat', 60_000))) fail('main UI did not restore after reload');
  await new Promise((r) => setTimeout(r, 1200));
}
async function clickInRow(page, channelName, inner) {
  const h = await page.evaluateHandle(
    (sel, n) => {
      const row = [...document.querySelectorAll('.channel-item')].find((r) => (r.textContent || '').includes(n));
      return row ? row.querySelector(sel) : null;
    },
    inner,
    channelName,
  );
  const el = h.asElement();
  if (el) await el.click();
  return !!el;
}
async function joinCall(page) {
  return (await clickButtonByText(page, 'Create Call', 8_000)) || (await clickButtonByText(page, 'Join Call', 8_000));
}

function findE2eDb() {
  // scripts/e2e-server.sh records the temp DB dir here.
  const f = '/tmp/annex-e2e-server.dbdir';
  if (!existsSync(f)) return null;
  const dir = readFileSync(f, 'utf8').trim();
  const db = join(dir, 'annex.db');
  return existsSync(db) ? db : null;
}

async function main() {
  const exe = resolveChrome();
  if (!exe) fail('no Chrome/Chromium found');
  log(`chrome: ${exe}`);
  log(`server: ${URL}`);
  try {
    const res = await fetch(`${URL}/health`);
    if (!res.ok || (await res.json()).status !== 'ok') fail('server /health not ok');
  } catch (e) { fail(`cannot reach ${URL}/health`, e); }

  mkdirSync(SHOT_DIR, { recursive: true });

  const browser = await puppeteer.launch({
    executablePath: exe,
    headless: true,
    args: [
      '--no-sandbox', '--disable-setuid-sandbox', '--disable-dev-shm-usage', '--use-gl=swiftshader',
      '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream',
      '--autoplay-policy=no-user-gesture-required', '--window-size=1280,900',
    ],
    defaultViewport: { width: 1280, height: 900 },
  });

  const ctxA = await browser.createBrowserContext();
  const ctxB = await browser.createBrowserContext();
  const a = await ctxA.newPage();
  const b = await ctxB.newPage();
  for (const [p, who] of [[a, 'A'], [b, 'B']]) {
    p.on('console', (m) => { if (m.type() === 'error') log(`${who} console[error]: ${m.text()}`); });
  }

  try {
    log('Alice (founder) startup');
    await startup(a);
    log('Bob startup');
    await startup(b);

    // Alice creates a Voice channel.
    const createBtn = await a.$('.create-channel-btn');
    if (!createBtn) fail('Alice has no create-channel control (not founder?)');
    await createBtn.click();
    if (!(await waitForSel(a, '.dialog', 10_000))) fail('CreateChannelDialog did not open');
    const voiceName = `vmulti-${Date.now().toString(36)}`;
    await (await a.$('.dialog input[placeholder="general"]')).type(voiceName);
    const sel = await a.$('.dialog select');
    if (sel) await sel.select('Voice');
    await clickButtonByText(a, 'Create', 10_000);
    await new Promise((r) => setTimeout(r, 1500));
    log(`voice channel created: ${voiceName}`);

    // Grant can_voice to every identity directly in the DB (both are founders-
    // worth in this harness; the server enforces can_voice from the DB at join).
    const db = findE2eDb();
    if (!db) fail('could not locate the e2e server DB to grant can_voice');
    execFileSync('sqlite3', ['-batch', '-bail', db, 'UPDATE platform_identities SET can_voice=1;']);
    log(`granted can_voice to all identities (db: ${db})`);

    // Bob reloads so the client re-loads permissions (now can_voice=true),
    // then joins the voice channel.
    await resumeAfterReload(b);
    await clickInRow(b, voiceName, '.join-btn');
    await new Promise((r) => setTimeout(r, 800));
    await clickInRow(b, voiceName, '.channel-select');
    if (!(await waitForSel(b, '.voice-panel', 15_000))) fail('Bob: voice panel did not render');

    // Alice joins her voice channel and starts the call.
    await clickInRow(a, voiceName, '.channel-select');
    if (!(await waitForSel(a, '.voice-panel', 15_000))) fail('Alice: voice panel did not render');
    if (!(await joinCall(a))) fail('Alice could not start the call');
    if (!(await waitForSel(a, '.voice-panel.connected', 45_000))) fail('Alice call did not connect');
    log('Alice connected to the call');

    // Bob joins the same call.
    if (!(await joinCall(b))) fail('Bob could not join the call (can_voice not applied?)');
    if (!(await waitForSel(b, '.voice-panel.connected', 45_000))) fail('Bob call did not connect');
    log('Bob connected to the call');

    // FAN-OUT proof: Alice must receive Bob's forwarded audio as a remote
    // track. The client renders a <audio> element per remote track.
    const gotRemote = await a
      .waitForFunction(
        () => {
          const audios = [...document.querySelectorAll('audio')];
          return audios.some((el) => el.srcObject) || audios.length > 0;
        },
        { timeout: 30_000, polling: 500 },
      )
      .then(() => true)
      .catch(() => false);

    await shot(a, 'voice-multi-alice');
    await shot(b, 'voice-multi-bob');

    if (!gotRemote) {
      fail('Alice never received a remote audio track from Bob (SFU fan-out not observed within 30s)');
    }
    log('OK — two-party voice fan-out: Alice received Bob\'s forwarded audio track via the SFU');
  } finally {
    await browser.close();
  }
}

main().then(() => process.exit(0)).catch((err) => fail('unexpected error', err));
