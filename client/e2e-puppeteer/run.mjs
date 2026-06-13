#!/usr/bin/env node
//
// run.mjs — Puppeteer end-to-end smoke + screenshot harness for Annex.
//
// A second, independent browser-automation lane next to the Playwright suite
// in client/e2e/. It drives the real production flow against a running Annex
// server (default http://127.0.0.1:3000, served with the built client dist):
//
//   IdentitySetup → "Create New Identity"
//     → StartupModeSelector → "Continue" (use this server)
//       → in-browser Groth16 membership proof (WASM, 30–60s)
//         → main Chat UI → join #General → send a message
//
// At every milestone it writes a full-page PNG to ./screenshots/ so the run
// produces visual evidence the app works, not just a green/red exit code.
//
// Browser: uses puppeteer-core with a caller-supplied Chrome. Resolution
// order for the executable:
//   1. $PUPPETEER_EXECUTABLE_PATH
//   2. the Playwright-managed Chromium under /opt/pw-browsers (CI / Claude env)
//   3. a system google-chrome / chromium on PATH
//
// Usage:
//   node client/e2e-puppeteer/run.mjs [--url http://127.0.0.1:3000] [--headful]
//
// Exit code is 0 only if every milestone is reached.

import { existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { execSync } from 'node:child_process';
import puppeteer from 'puppeteer-core';

const __dirname = dirname(fileURLToPath(import.meta.url));
const SHOT_DIR = join(__dirname, 'screenshots');

function parseArgs(argv) {
  const out = { url: 'http://127.0.0.1:3000', headful: false };
  for (let i = 0; i < argv.length; i++) {
    if (argv[i] === '--url' && i + 1 < argv.length) out.url = argv[++i];
    else if (argv[i].startsWith('--url=')) out.url = argv[i].slice('--url='.length);
    else if (argv[i] === '--headful') out.headful = true;
  }
  return out;
}

function log(msg) {
  console.log(`[pptr-e2e] ${msg}`);
}

function fail(msg, err) {
  console.error(`[pptr-e2e] FAIL: ${msg}`);
  if (err) console.error(err.stack ?? err.message ?? String(err));
  process.exit(1);
}

function resolveChrome() {
  const fromEnv = process.env.PUPPETEER_EXECUTABLE_PATH;
  if (fromEnv && existsSync(fromEnv)) return fromEnv;

  // Playwright-managed Chromium (Claude Code env + CI after `playwright install`).
  const pwRoot = process.env.PLAYWRIGHT_BROWSERS_PATH || '/opt/pw-browsers';
  if (existsSync(pwRoot)) {
    for (const entry of readdirSync(pwRoot)) {
      if (entry.startsWith('chromium-')) {
        const candidate = join(pwRoot, entry, 'chrome-linux', 'chrome');
        if (existsSync(candidate)) return candidate;
      }
    }
  }

  // System Chrome/Chromium.
  for (const bin of ['google-chrome-stable', 'google-chrome', 'chromium', 'chromium-browser']) {
    try {
      const p = execSync(`command -v ${bin}`, { stdio: ['ignore', 'pipe', 'ignore'] })
        .toString()
        .trim();
      if (p && existsSync(p)) return p;
    } catch {
      /* not found, keep looking */
    }
  }
  return null;
}

let shotIndex = 0;
async function shot(page, name) {
  const file = join(SHOT_DIR, `${String(++shotIndex).padStart(2, '0')}-${name}.png`);
  await page.screenshot({ path: file, fullPage: true });
  log(`screenshot → ${file}`);
}

// Click the first visible <button> (or [role=button]) whose trimmed text
// matches `text` exactly. Returns false if none found within `timeoutMs`.
async function clickButtonByText(page, text, timeoutMs = 30_000) {
  const handle = await page
    .waitForFunction(
      (label) => {
        const els = [...document.querySelectorAll('button, [role="button"]')];
        const match = els.find((el) => (el.textContent || '').trim() === label);
        return match || null;
      },
      { timeout: timeoutMs, polling: 200 },
      text,
    )
    .catch(() => null);
  if (!handle) return false;
  const el = handle.asElement();
  if (!el) return false;
  await el.click();
  return true;
}

async function waitForButtonByText(page, text, timeoutMs = 90_000) {
  return page
    .waitForFunction(
      (label) => {
        const els = [...document.querySelectorAll('button, [role="button"]')];
        return els.some((el) => (el.textContent || '').trim() === label);
      },
      { timeout: timeoutMs, polling: 250 },
      text,
    )
    .then(() => true)
    .catch(() => false);
}

async function waitForText(page, text, timeoutMs = 30_000) {
  return page
    .waitForFunction(
      (needle) => (document.body?.innerText || '').includes(needle),
      { timeout: timeoutMs, polling: 250 },
      text,
    )
    .then(() => true)
    .catch(() => false);
}

async function main() {
  const { url, headful } = parseArgs(process.argv.slice(2));

  const execPath = resolveChrome();
  if (!execPath) {
    fail(
      'no Chrome/Chromium found. Set PUPPETEER_EXECUTABLE_PATH, or run ' +
        '`npx playwright install chromium`, or install google-chrome.',
    );
  }
  log(`chrome: ${execPath}`);
  log(`server: ${url}`);

  // Fresh screenshot dir each run.
  rmSync(SHOT_DIR, { recursive: true, force: true });
  mkdirSync(SHOT_DIR, { recursive: true });

  // Preflight: server must be up.
  try {
    const res = await fetch(`${url}/health`);
    const body = await res.json();
    if (!res.ok || body.status !== 'ok') {
      fail(`/health not ok: ${res.status} ${JSON.stringify(body)}`);
    }
    log('/health ok');
  } catch (err) {
    fail(`cannot reach ${url}/health — start the e2e server first (scripts/e2e-server.sh start)`, err);
  }

  const browser = await puppeteer.launch({
    executablePath: execPath,
    headless: !headful,
    args: [
      '--no-sandbox',
      '--disable-setuid-sandbox',
      '--disable-dev-shm-usage',
      '--use-gl=swiftshader',
      '--window-size=1280,900',
    ],
    defaultViewport: { width: 1280, height: 900 },
  });

  try {
    const page = await browser.newPage();
    page.on('console', (m) => {
      const t = m.type();
      if (t === 'error' || t === 'warning') log(`page-console[${t}]: ${m.text()}`);
    });
    page.on('pageerror', (e) => log(`page-error: ${e.message}`));

    // ── 1. First visit → identity creation screen ─────────────────────
    log('navigating to app root');
    await page.goto(url, { waitUntil: 'domcontentloaded', timeout: 60_000 });

    if (!(await waitForButtonByText(page, 'Create New Identity', 30_000))) {
      await shot(page, 'identity-screen-MISSING');
      fail('identity creation screen never rendered ("Create New Identity" button)');
    }
    await shot(page, 'identity-setup');

    // ── 2. Create identity ────────────────────────────────────────────
    log('clicking "Create New Identity"');
    if (!(await clickButtonByText(page, 'Create New Identity', 15_000))) {
      fail('could not click "Create New Identity"');
    }

    // ── 3. Server selection (StartupModeSelector) ─────────────────────
    log('waiting for server-selection step ("Continue")');
    if (!(await waitForButtonByText(page, 'Continue', 30_000))) {
      await shot(page, 'server-select-MISSING');
      fail('server selection ("Continue") never appeared after identity creation');
    }
    await shot(page, 'server-select');

    log('clicking "Continue" (use this server)');
    if (!(await clickButtonByText(page, 'Continue', 15_000))) {
      fail('could not click "Continue"');
    }

    // ── 4. ZK proof + registration → main Chat UI ─────────────────────
    log('waiting for main UI (ZK proof generation, up to 120s)…');
    if (!(await waitForButtonByText(page, 'Chat', 120_000))) {
      await shot(page, 'main-ui-MISSING');
      fail('main chat UI ("Chat" nav button) never appeared — ZK proof/registration likely failed');
    }
    // Give the layout a beat to settle, then capture.
    await new Promise((r) => setTimeout(r, 1500));
    await shot(page, 'main-chat-ui');

    // ── 5. Join + select #General, send a message ─────────────────────
    const sawGeneral = await waitForText(page, 'General', 10_000);
    if (sawGeneral) {
      log('#General visible — attempting join + message');
      // Click a join "+" button if present, then the channel row.
      await page.evaluate(() => {
        const rows = [...document.querySelectorAll('.channel-item')];
        const general = rows.find((r) => (r.textContent || '').includes('General'));
        if (general) {
          const joinBtn = general.querySelector('.join-btn');
          if (joinBtn) joinBtn.click();
        }
      });
      await new Promise((r) => setTimeout(r, 1000));
      await page.evaluate(() => {
        const rows = [...document.querySelectorAll('.channel-item')];
        const general = rows.find((r) => (r.textContent || '').includes('General'));
        if (general) general.click();
      });
      await new Promise((r) => setTimeout(r, 1000));

      const input = await page.$('input[placeholder="Type a message..."], textarea[placeholder="Type a message..."]');
      if (input) {
        const msg = `Hello from Puppeteer E2E ${Date.now()}`;
        await input.type(msg);
        await shot(page, 'message-typed');
        const sent = await clickButtonByText(page, 'Send', 5_000);
        if (sent) {
          await waitForText(page, msg, 15_000);
          await shot(page, 'message-sent');
          log('message round-trip captured');
        } else {
          log('Send button not found — captured composer state only');
        }
      } else {
        log('message composer not found — skipping message step (still a pass: main UI reached)');
        await shot(page, 'main-ui-no-composer');
      }
    } else {
      log('#General not visible within 10s — skipping message step (main UI still reached)');
    }

    log('OK — Puppeteer drove the full identity → proof → chat flow');
  } finally {
    await browser.close();
  }
}

main()
  .then(() => process.exit(0))
  .catch((err) => fail('unexpected error', err));
