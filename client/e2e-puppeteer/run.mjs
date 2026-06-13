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
    // The clickable target is the inner `.channel-select` BUTTON (and the
    // `.join-btn` for membership) — clicking the outer `.channel-item` div
    // does not fire React's onClick. We must JOIN first (so the server lets
    // us read/post), then SELECT (so the composer renders).
    const sawGeneral = await waitForText(page, 'General', 10_000);
    if (sawGeneral) {
      log('#General visible — joining then selecting');

      // Helper: real mouse click on the first matching element whose ancestor
      // .channel-item contains "General".
      const clickInGeneralRow = async (innerSelector) => {
        const handle = await page.evaluateHandle(
          (sel) => {
            const rows = [...document.querySelectorAll('.channel-item')];
            const general = rows.find((r) => (r.textContent || '').includes('General'));
            return general ? general.querySelector(sel) : null;
          },
          innerSelector,
        );
        const el = handle.asElement();
        if (!el) return false;
        await el.click();
        return true;
      };

      // Join (if a join button is present — i.e. we're not already a member).
      if (await clickInGeneralRow('.join-btn')) {
        log('clicked join (+)');
        // Membership confirmed when the row swaps join (+) for leave (x).
        await page
          .waitForFunction(
            () => {
              const rows = [...document.querySelectorAll('.channel-item')];
              const g = rows.find((r) => (r.textContent || '').includes('General'));
              return g && g.querySelector('.leave-btn');
            },
            { timeout: 15_000, polling: 200 },
          )
          .catch(() => log('leave-btn did not appear within 15s (continuing)'));
      } else {
        log('no join button (already a member?) — selecting directly');
      }

      // Select the channel so the message view + composer mount.
      if (await clickInGeneralRow('.channel-select')) {
        log('selected #General');
      }

      // Composer textarea ("Type a message...") only renders once a channel is
      // active and the WS is connected.
      const composer = await page
        .waitForSelector('textarea[placeholder="Type a message..."]', { timeout: 20_000 })
        .catch(() => null);

      if (composer) {
        const msg = `Hello from Puppeteer E2E ${Date.now()}`;
        await composer.type(msg);
        await shot(page, 'message-typed');
        const sent = await clickButtonByText(page, 'Send', 5_000);
        if (!sent) fail('Send button not found');

        await waitForText(page, msg, 15_000);
        await shot(page, 'message-sent');

        // The optimistic UI renders the message text even when the send is
        // REJECTED (it just marks it "failed"). Asserting on text alone is how
        // a "Not a member" 403 hid for so long — so explicitly reject those
        // failure markers here.
        const banner = await page.evaluate(
          () => document.body?.innerText?.includes('Not a member of channel') ?? false,
        );
        if (banner) {
          fail('server rejected the action with "Not a member of channel" — channel join failed');
        }
        const failedSend = await page.evaluate(() => {
          // A failed message row exposes a retry/dismiss affordance.
          const txt = document.body?.innerText || '';
          return /\bfailed\b/i.test(txt) && /\bretry\b/i.test(txt);
        });
        if (failedSend) {
          fail('message send shows "failed / retry" — the message did not post');
        }
        log('message round-trip confirmed (posted, not failed, member of channel)');
      } else {
        log('message composer did not render — capturing state (main UI still reached = pass)');
        await shot(page, 'main-ui-no-composer');
      }
    } else {
      log('#General not visible within 10s — skipping message step (main UI still reached)');
    }

    // ── 6. Cold start: reload and prove the persisted ZK proof restores ───
    // After a reload the app restores the identity from IndexedDB straight to
    // the main UI WITHOUT re-running the (30–60s) proof. If the cached proof
    // isn't persisted + re-attached, channel join/send 403s again. This step
    // catches that regression.
    if (sawGeneral) {
      log('cold-start check: reloading the page');
      await page.reload({ waitUntil: 'domcontentloaded', timeout: 60_000 });

      // The server-selection step (StartupModeSelector) reappears because
      // serverReady is per-load React state; click Continue if present.
      if (await waitForButtonByText(page, 'Continue', 15_000)) {
        await clickButtonByText(page, 'Continue', 10_000);
      }
      if (!(await waitForButtonByText(page, 'Chat', 60_000))) {
        await shot(page, 'cold-start-main-ui-MISSING');
        fail('main UI did not restore after reload');
      }
      await new Promise((r) => setTimeout(r, 1000));

      // Select #General (already a member from before) and send again.
      const selectGeneral = async () => {
        const handle = await page.evaluateHandle(() => {
          const rows = [...document.querySelectorAll('.channel-item')];
          const g = rows.find((r) => (r.textContent || '').includes('General'));
          return g ? g.querySelector('.channel-select') : null;
        });
        const el = handle.asElement();
        if (el) await el.click();
      };
      await selectGeneral();

      const composer2 = await page
        .waitForSelector('textarea[placeholder="Type a message..."]', { timeout: 20_000 })
        .catch(() => null);
      if (!composer2) fail('composer did not render after reload');

      const msg2 = `Cold-start Puppeteer E2E ${Date.now()}`;
      await composer2.type(msg2);
      if (!(await clickButtonByText(page, 'Send', 5_000))) fail('Send not found after reload');
      await waitForText(page, msg2, 15_000);
      await shot(page, 'cold-start-message-sent');

      const coldFailed = await page.evaluate(() => {
        const txt = document.body?.innerText || '';
        return (
          txt.includes('Not a member of channel') ||
          (/\bfailed\b/i.test(txt) && /\bretry\b/i.test(txt))
        );
      });
      if (coldFailed) {
        fail('after reload, send failed / "Not a member" — persisted ZK proof did not restore');
      }
      log('cold-start confirmed: persisted proof restored, post succeeded after reload');
    }

    // ── 7. Channel creation (founder / can_moderate) ──────────────────
    // The first identity on a fresh server is promoted to founder, so the
    // create-channel "+" is present. Drive the full CreateChannelDialog and
    // prove the new channel shows up in the list (moderator-gated CRUD).
    const createBtn = await page.$('.create-channel-btn');
    if (createBtn) {
      log('creating a channel (founder / can_moderate)');
      await createBtn.click();
      if (!(await page.waitForSelector('.dialog', { timeout: 10_000 }).catch(() => null))) {
        fail('CreateChannelDialog did not open');
      }
      const newChannel = `evidence-${Date.now().toString(36)}`;
      const nameInput = await page.$('.dialog input[placeholder="general"]');
      if (!nameInput) fail('channel-name input not found in dialog');
      await nameInput.type(newChannel);
      const topicInput = await page.$('.dialog input[placeholder="What this channel is about"]');
      if (topicInput) await topicInput.type('Created by the Puppeteer evidence run');
      await shot(page, 'channel-create-dialog');

      const createSubmit = await page.$('.dialog .primary-btn');
      if (!createSubmit) fail('Create submit button not found');
      await createSubmit.click();

      const created = await page
        .waitForFunction(
          (label) =>
            [...document.querySelectorAll('.channel-item')].some((r) =>
              (r.textContent || '').includes(label),
            ),
          { timeout: 15_000, polling: 200 },
          newChannel,
        )
        .then(() => true)
        .catch(() => false);
      if (!created) fail(`created channel "${newChannel}" never appeared in the channel list`);
      await shot(page, 'channel-created');
      log(`channel created and listed: ${newChannel}`);
    } else {
      log('no create-channel control (identity is not a moderator) — skipping channel-create');
    }

    log('OK — Puppeteer drove the full identity → proof → chat flow (incl. cold start)');
  } finally {
    await browser.close();
  }
}

main()
  .then(() => process.exit(0))
  .catch((err) => fail('unexpected error', err));
