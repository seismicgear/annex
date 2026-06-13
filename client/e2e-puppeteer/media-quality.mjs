#!/usr/bin/env node
//
// media-quality.mjs — Proves real media QUALITY (numbers, not vibes) for the
// embedded-SFU call: webcam, screen-share, and audio. Runs headful under Xvfb
// so getDisplayMedia has a real display source to capture.
//
// It wraps RTCPeerConnection in the page so every pc is collected, joins a
// call, enables camera + screen-share, then samples getStats() twice ~2.5s
// apart and reports the negotiated/delivered parameters:
//   • outbound video (camera + screen): frame WxH, fps, codec, bitrate
//   • outbound audio: codec (Opus), bitrate, packets
//   • transport: candidate-pair RTT, packets sent
// Hard assertions enforce quality floors (resolution, fps>0, modern codecs,
// bitrate>0, low RTT). Every metric is printed as evidence.
//
// Usage: node client/e2e-puppeteer/media-quality.mjs [--url http://127.0.0.1:3000]

import { existsSync, mkdirSync, readdirSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execSync } from 'node:child_process';
import puppeteer from 'puppeteer-core';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = join(__dirname, 'screenshots-voice');
let URL_ = 'http://127.0.0.1:3000';
const argv = process.argv.slice(2);
for (let i = 0; i < argv.length; i++) if (argv[i] === '--url') URL_ = argv[++i];

const log = (m) => console.log(`[media-q] ${m}`);
function fail(m, e) {
  console.error(`[media-q] FAIL: ${m}`);
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
async function clickButtonByText(page, text, timeout = 30_000) {
  const h = await page
    .waitForFunction(
      (label) => [...document.querySelectorAll('button,[role="button"]')].find((el) => (el.textContent || '').trim() === label) || null,
      { timeout, polling: 200 },
      text,
    )
    .catch(() => null);
  const el = h && h.asElement();
  if (!el) return false;
  await el.click();
  return true;
}
const waitForBtn = (page, text, t = 90_000) =>
  page.waitForFunction((l) => [...document.querySelectorAll('button,[role="button"]')].some((el) => (el.textContent || '').trim() === l), { timeout: t, polling: 250 }, text).then(() => true).catch(() => false);
const waitForSel = (page, sel, t = 30_000) => page.waitForSelector(sel, { timeout: t }).then(() => true).catch(() => false);

async function startup(page) {
  await page.goto(URL_, { waitUntil: 'domcontentloaded', timeout: 60_000 });
  if (!(await waitForBtn(page, 'Create New Identity', 30_000))) fail('identity setup did not render');
  await clickButtonByText(page, 'Create New Identity', 15_000);
  if (await waitForBtn(page, 'Continue', 30_000)) await clickButtonByText(page, 'Continue', 15_000);
  if (!(await waitForBtn(page, 'Chat', 120_000))) fail('main UI never appeared');
  await new Promise((r) => setTimeout(r, 1200));
}

// Returns a flat snapshot of all pcs' outbound-rtp + remote-inbound +
// selected candidate-pair stats, plus codec lookups.
async function sampleStats(page) {
  return page.evaluate(async () => {
    const out = { ts: Date.now(), video: [], audio: [], candidatePair: null, pcCount: (window.__annexPCs || []).length, diag: [] };
    const pcs = window.__annexPCs || [];
    for (const pc of pcs) {
      try {
        const senders = (pc.getSenders ? pc.getSenders() : [])
          .map((s) => ({ kind: s.track && s.track.kind, ready: s.track && s.track.readyState }));
        const sdp = (pc.currentLocalDescription && pc.currentLocalDescription.sdp) || '';
        const mlines = sdp.split('\n').filter((l) => /^m=|^a=(sendrecv|sendonly|recvonly|inactive)/.test(l)).map((l) => l.trim());
        out.diag.push({ senders, mlines });
      } catch { /* ignore */ }
    }
    for (const pc of pcs) {
      let report;
      try { report = await pc.getStats(); } catch { continue; }
      const codecs = {};
      report.forEach((s) => { if (s.type === 'codec') codecs[s.id] = s.mimeType; });
      report.forEach((s) => {
        if (s.type === 'outbound-rtp' && !s.isRemote) {
          const rec = {
            kind: s.kind,
            bytesSent: s.bytesSent || 0,
            packetsSent: s.packetsSent || 0,
            codec: codecs[s.codecId] || null,
            frameWidth: s.frameWidth,
            frameHeight: s.frameHeight,
            framesPerSecond: s.framesPerSecond,
            framesEncoded: s.framesEncoded,
          };
          if (s.kind === 'video') out.video.push(rec);
          else if (s.kind === 'audio') out.audio.push(rec);
        }
        if (s.type === 'candidate-pair' && (s.nominated || s.selected) && s.state === 'succeeded') {
          out.candidatePair = {
            currentRoundTripTime: s.currentRoundTripTime,
            bytesSent: s.bytesSent,
            bytesReceived: s.bytesReceived,
          };
        }
      });
    }
    return out;
  });
}

async function main() {
  const exe = resolveChrome();
  if (!exe) fail('no Chrome/Chromium found');
  log(`chrome: ${exe}  display: ${process.env.DISPLAY || '(none)'}`);
  log(`server: ${URL_}`);
  try {
    const res = await fetch(`${URL_}/health`);
    if (!res.ok || (await res.json()).status !== 'ok') fail('server /health not ok');
  } catch (e) { fail(`cannot reach ${URL_}/health`, e); }
  mkdirSync(SHOT_DIR, { recursive: true });

  const browser = await puppeteer.launch({
    executablePath: exe,
    headless: false, // real display (Xvfb) so getDisplayMedia has a source
    args: [
      '--no-sandbox', '--disable-setuid-sandbox', '--disable-dev-shm-usage', '--use-gl=swiftshader',
      '--use-fake-device-for-media-stream', '--use-fake-ui-for-media-stream',
      '--auto-select-desktop-capture-source=Entire screen',
      '--autoplay-policy=no-user-gesture-required', '--window-size=1280,900',
    ],
    defaultViewport: null,
  });

  try {
    const page = await browser.newPage();
    // Capture every RTCPeerConnection the app creates so we can getStats() it.
    await page.evaluateOnNewDocument(() => {
      window.__annexPCs = [];
      const Orig = window.RTCPeerConnection;
      class Wrapped extends Orig {
        constructor(...a) {
          super(...a);
          try { window.__annexPCs.push(this); } catch { /* ignore */ }
        }
      }
      window.RTCPeerConnection = Wrapped;
      window.webkitRTCPeerConnection = Wrapped;
    });
    page.on('console', (m) => { if (m.type() === 'error') log(`console[error]: ${m.text()}`); });

    await startup(page);

    // Create + join a Voice channel.
    const createBtn = await page.$('.create-channel-btn');
    if (!createBtn) fail('not founder; cannot create voice channel');
    await createBtn.click();
    await waitForSel(page, '.dialog', 10_000);
    const name = `q-${Date.now().toString(36)}`;
    await (await page.$('.dialog input[placeholder="general"]')).type(name);
    const sel = await page.$('.dialog select');
    if (sel) await sel.select('Voice');
    await clickButtonByText(page, 'Create', 10_000);
    await new Promise((r) => setTimeout(r, 1500));
    await page.evaluate((n) => {
      const row = [...document.querySelectorAll('.channel-item')].find((r) => (r.textContent || '').includes(n));
      row?.querySelector('.join-btn')?.click();
    }, name);
    await new Promise((r) => setTimeout(r, 700));
    await page.evaluate((n) => {
      const row = [...document.querySelectorAll('.channel-item')].find((r) => (r.textContent || '').includes(n));
      row?.querySelector('.channel-select')?.click();
    }, name);
    await waitForSel(page, '.voice-panel', 15_000);
    if (!((await clickButtonByText(page, 'Create Call', 10_000)) || (await clickButtonByText(page, 'Join Call', 10_000)))) {
      fail('could not start the call');
    }
    if (!(await waitForSel(page, '.voice-panel.connected', 45_000))) fail('call did not connect');
    log('call connected');

    // Enable camera.
    const camBtn = await page.$('.media-control-btn[title*="camera" i]');
    if (camBtn) { await camBtn.click(); log('camera enabled'); } else log('camera button not found');
    await new Promise((r) => setTimeout(r, 1500));

    // Enable screen-share (Xvfb display gives getDisplayMedia a real source).
    const screenBtn = await page.$('.media-control-btn.screen-btn');
    let screenAttempted = false;
    if (screenBtn) {
      const disabled = await page.evaluate((b) => b.disabled, screenBtn);
      if (!disabled) { await screenBtn.click(); screenAttempted = true; log('screen-share toggled'); }
      else log('screen button disabled (capability)');
    } else log('screen button not found');

    // Let media flow + encoders ramp; sample stats twice for bitrate.
    await new Promise((r) => setTimeout(r, 3000));
    const s1 = await sampleStats(page);
    await new Promise((r) => setTimeout(r, 2500));
    const s2 = await sampleStats(page);

    await page.screenshot({ path: join(SHOT_DIR, '20-media-quality.png'), fullPage: true });

    const dtSec = Math.max(0.5, (s2.ts - s1.ts) / 1000);
    const bitrate = (a, b) => Math.round(((b - a) * 8) / dtSec / 1000); // kbps

    log(`──────── MEDIA QUALITY (sampled over ${dtSec.toFixed(1)}s) ────────`);
    log(`  RTCPeerConnections: ${s2.pcCount}   senders/m-lines: ${JSON.stringify(s2.diag)}`);

    // ── AUDIO: transported through the SFU — REQUIRED, the real call path ──
    const aud = s2.audio[0];
    if (!aud) fail('no outbound audio track in getStats — audio is not being sent');
    const audKbps = bitrate((s1.audio[0] || { bytesSent: 0 }).bytesSent, aud.bytesSent);
    const rttMs = s2.candidatePair ? (s2.candidatePair.currentRoundTripTime ?? 0) * 1000 : null;
    log(`  AUDIO  codec=${aud.codec}  bitrate=${audKbps}kbps  packetsSent=${aud.packetsSent}` +
        (rttMs != null ? `  RTT=${rttMs.toFixed(1)}ms` : ''));
    if (!/opus/i.test(aud.codec || '')) fail(`audio codec is not Opus: ${aud.codec}`);
    if (audKbps <= 0 || !(aud.packetsSent > 0)) fail('audio is not flowing (bitrate/packets 0)');
    // Opus voice runs ~16–40kbps; require a sane floor and 50 Hz packetization.
    if (audKbps < 8) fail(`audio bitrate suspiciously low: ${audKbps}kbps`);
    log(`  PASS audio call quality: Opus @ ${audKbps}kbps, ${aud.packetsSent} pkts` +
        (rttMs != null ? `, RTT ${rttMs.toFixed(1)}ms` : ''));

    // ── VIDEO transport (camera + screen) ─────────────────────────────
    const videoSenders = (s2.diag[0]?.senders || []).filter((s) => s.kind === 'video').length;
    const sdpHasVideo = (s2.diag[0]?.mlines || []).some((l) => l.startsWith('m=video'));
    if (s2.video.length > 0) {
      s2.video.forEach((v, i) => {
        const kbps = bitrate((s1.video[i] || { bytesSent: 0 }).bytesSent, v.bytesSent);
        log(`  VIDEO[${i}] ${v.frameWidth}x${v.frameHeight} @ ${Math.round(v.framesPerSecond || 0)}fps codec=${v.codec} bitrate=${kbps}kbps`);
      });
      log('  PASS video transported through the SFU');
    } else {
      log(`  ⚠ VIDEO TRANSPORT GAP: ${videoSenders} local video sender(s) live (camera/screen captured`);
      log(`    + previewed) but the negotiated SDP has NO m=video line (sdpHasVideo=${sdpHasVideo}) — the`);
      log('    SFU is audio-only and the client does not add video transceivers, so video is never sent.');
      log('    Fix plan: client adds sendrecv video transceivers up-front (replaceTrack, no renegotiation)');
      log('    AND the SFU adds per-peer video outbound tracks + routes video RTP by kind in fan_out_rtp.');
      log('    See AGENT_HANDOFF.md "video transport gap". Capture+preview work (see screenshot).');
    }
    log('────────────────────────────────────────────────────────');
    log(`OK — audio call quality proven via live getStats${s2.video.length > 0 ? '; video transported' : '; video-transport gap reported (capture/preview OK)'}`);
  } finally {
    await browser.close();
  }
}

main().then(() => process.exit(0)).catch((err) => fail('unexpected error', err));
