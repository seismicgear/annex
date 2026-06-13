#!/usr/bin/env node
//
// voice-video.mjs — Two-party VIDEO fan-out proof. Alice + Bob both join the
// call with cameras on; each must RECEIVE the other's camera via the SFU,
// proven by inbound-rtp video in getStats (bytesReceived/frames > 0). This is
// the end-to-end webcam-quality proof (encode → SFU forward → decode on the
// far side), with real resolution/fps/bitrate numbers on the receiver.
//
// Cameras use Chromium's fake device, so no display/Xvfb is needed.
// Usage: node client/e2e-puppeteer/voice-video.mjs [--url http://127.0.0.1:3000]

import { existsSync, readdirSync, readFileSync, mkdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execSync, execFileSync } from 'node:child_process';
import puppeteer from 'puppeteer-core';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = join(__dirname, 'screenshots-voice');
let URL_ = 'http://127.0.0.1:3000';
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) if (argv[i] === '--url') URL_ = argv[++i];

const log = (m) => console.log(`[voice-video] ${m}`);
function fail(m, e) { console.error(`[voice-video] FAIL: ${m}`); if (e) console.error(e.stack ?? e.message ?? String(e)); process.exit(1); }
function resolveChrome() {
  const env = process.env.PUPPETEER_EXECUTABLE_PATH;
  if (env && existsSync(env)) return env;
  const pwRoot = process.env.PLAYWRIGHT_BROWSERS_PATH || '/opt/pw-browsers';
  if (existsSync(pwRoot)) for (const e of readdirSync(pwRoot)) if (e.startsWith('chromium-')) { const c = join(pwRoot, e, 'chrome-linux', 'chrome'); if (existsSync(c)) return c; }
  for (const b of ['google-chrome-stable', 'google-chrome', 'chromium', 'chromium-browser']) { try { const p = execSync(`command -v ${b}`, { stdio: ['ignore', 'pipe', 'ignore'] }).toString().trim(); if (p && existsSync(p)) return p; } catch { /* */ } }
  return null;
}
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function clickByText(page, text, t = 30_000) {
  const h = await page.waitForFunction((l) => [...document.querySelectorAll('button,[role="button"]')].find((el) => (el.textContent || '').trim() === l) || null, { timeout: t, polling: 200 }, text).catch(() => null);
  const el = h && h.asElement(); if (!el) return false; await el.click(); return true;
}
const waitForBtn = (p, t, to = 90_000) => p.waitForFunction((l) => [...document.querySelectorAll('button,[role="button"]')].some((el) => (el.textContent || '').trim() === l), { timeout: to, polling: 250 }, t).then(() => true).catch(() => false);
const waitForSel = (p, s, to = 20_000) => p.waitForSelector(s, { timeout: to }).then(() => true).catch(() => false);
async function instrument(page) {
  await page.evaluateOnNewDocument(() => {
    window.__pcs = [];
    const O = window.RTCPeerConnection;
    class W extends O { constructor(...a) { super(...a); try { window.__pcs.push(this); } catch { /* */ } } }
    window.RTCPeerConnection = W; window.webkitRTCPeerConnection = W;
  });
}
async function startup(page) {
  await page.goto(URL_, { waitUntil: 'domcontentloaded', timeout: 60_000 });
  if (!(await waitForBtn(page, 'Create New Identity', 30_000))) fail('identity setup did not render');
  await clickByText(page, 'Create New Identity', 15_000);
  if (await waitForBtn(page, 'Continue', 30_000)) await clickByText(page, 'Continue', 15_000);
  if (!(await waitForBtn(page, 'Chat', 120_000))) fail('main UI never appeared');
  await sleep(1200);
}
async function resume(page) {
  await page.reload({ waitUntil: 'domcontentloaded', timeout: 60_000 });
  if (await waitForBtn(page, 'Continue', 20_000)) await clickByText(page, 'Continue', 15_000);
  if (!(await waitForBtn(page, 'Chat', 60_000))) fail('main UI did not restore after reload');
  await sleep(1200);
}
async function clickRow(page, name, inner) {
  const h = await page.evaluateHandle((sel, n) => { const row = [...document.querySelectorAll('.channel-item')].find((r) => (r.textContent || '').includes(n)); return row ? row.querySelector(sel) : null; }, inner, name);
  const el = h.asElement(); if (el) await el.click(); return !!el;
}
const joinCall = (p) => clickByText(p, 'Create Call', 8_000).then((ok) => ok || clickByText(p, 'Join Call', 8_000));
function findDb() { const f = '/tmp/annex-e2e-server.dbdir'; if (!existsSync(f)) return null; const db = join(readFileSync(f, 'utf8').trim(), 'annex.db'); return existsSync(db) ? db : null; }
// Inbound video stats on the receiver.
async function inboundVideo(page) {
  return page.evaluate(async () => {
    const res = [];
    for (const pc of (window.__pcs || [])) {
      let rep; try { rep = await pc.getStats(); } catch { continue; }
      const codecs = {}; rep.forEach((s) => { if (s.type === 'codec') codecs[s.id] = s.mimeType; });
      rep.forEach((s) => { if (s.type === 'inbound-rtp' && s.kind === 'video') res.push({ bytesReceived: s.bytesReceived || 0, frameWidth: s.frameWidth, frameHeight: s.frameHeight, framesPerSecond: s.framesPerSecond, framesDecoded: s.framesDecoded, codec: codecs[s.codecId] || null }); });
    }
    return res;
  });
}

async function main() {
  const exe = resolveChrome(); if (!exe) fail('no Chrome');
  try { const r = await fetch(`${URL_}/health`); if (!r.ok || (await r.json()).status !== 'ok') fail('health not ok'); } catch (e) { fail('cannot reach server', e); }
  mkdirSync(SHOT_DIR, { recursive: true });
  const browser = await puppeteer.launch({ executablePath: exe, headless: true, args: ['--no-sandbox', '--disable-setuid-sandbox', '--disable-dev-shm-usage', '--use-gl=swiftshader', '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream', '--autoplay-policy=no-user-gesture-required', '--window-size=1280,900'], defaultViewport: { width: 1280, height: 900 } });
  const ctxA = await browser.createBrowserContext(); const ctxB = await browser.createBrowserContext();
  const a = await ctxA.newPage(); const b = await ctxB.newPage();
  await instrument(a); await instrument(b);
  try {
    log('Alice startup'); await startup(a);
    log('Bob startup'); await startup(b);
    // Alice creates a Voice channel.
    const cb = await a.$('.create-channel-btn'); if (!cb) fail('Alice not founder');
    await cb.click(); await waitForSel(a, '.dialog', 10_000);
    const name = `vv-${Date.now().toString(36)}`;
    await (await a.$('.dialog input[placeholder="general"]')).type(name);
    const sel = await a.$('.dialog select'); if (sel) await sel.select('Voice');
    await clickByText(a, 'Create', 10_000); await sleep(1500);
    // Grant can_voice to all.
    const db = findDb(); if (!db) fail('cannot find e2e db');
    execFileSync('sqlite3', ['-batch', '-bail', db, 'UPDATE platform_identities SET can_voice=1;']);
    log('granted can_voice');
    // Bob reload → join voice channel.
    await resume(b);
    await clickRow(b, name, '.join-btn'); await sleep(800); await clickRow(b, name, '.channel-select');
    if (!(await waitForSel(b, '.voice-panel', 15_000))) fail('Bob voice panel missing');
    // Alice select + join.
    await clickRow(a, name, '.channel-select');
    if (!(await waitForSel(a, '.voice-panel', 15_000))) fail('Alice voice panel missing');
    if (!(await joinCall(a))) fail('Alice cannot start call');
    if (!(await waitForSel(a, '.voice-panel.connected', 45_000))) fail('Alice not connected');
    if (!(await joinCall(b))) fail('Bob cannot join call');
    if (!(await waitForSel(b, '.voice-panel.connected', 45_000))) fail('Bob not connected');
    log('both connected');
    // Both enable cameras.
    const cam = async (p, who) => { const btn = await p.$('.media-control-btn[title*="camera" i]'); if (!btn) fail(`${who}: camera button missing`); await btn.click(); };
    await cam(a, 'Alice'); await cam(b, 'Bob');
    log('cameras enabled on both — waiting for SFU video fan-out…');
    await sleep(6000);
    // Each must RECEIVE the other's camera via the SFU.
    const va = await inboundVideo(a); const vb = await inboundVideo(b);
    log(`Alice inbound video: ${JSON.stringify(va)}`);
    log(`Bob   inbound video: ${JSON.stringify(vb)}`);
    await a.screenshot({ path: join(SHOT_DIR, '30-video-multi-alice.png'), fullPage: true });
    const recv = (arr) => arr.find((v) => (v.bytesReceived > 0) || (v.framesDecoded > 0));
    const ra = recv(va); const rb = recv(vb);
    if (!ra) fail('Alice did NOT receive Bob\'s video via the SFU (no inbound video bytes/frames)');
    if (!rb) fail('Bob did NOT receive Alice\'s video via the SFU');
    log(`PASS: Alice receives video ${ra.frameWidth || '?'}x${ra.frameHeight || '?'} codec=${ra.codec} framesDecoded=${ra.framesDecoded} bytes=${ra.bytesReceived}`);
    log(`PASS: Bob receives video   ${rb.frameWidth || '?'}x${rb.frameHeight || '?'} codec=${rb.codec} framesDecoded=${rb.framesDecoded} bytes=${rb.bytesReceived}`);
    log('OK — two-party VIDEO fan-out proven (each peer decodes the other\'s camera forwarded by the SFU)');
  } finally { await browser.close(); }
}
main().then(() => process.exit(0)).catch((e) => fail('unexpected', e));
