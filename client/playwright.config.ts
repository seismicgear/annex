import { defineConfig, devices } from '@playwright/test';

/**
 * Playwright E2E test configuration for Annex.
 *
 * Expects the Axum server to be running at http://127.0.0.1:3000 with the
 * built client dist served. Start it before running tests:
 *
 *   cd .. && ANNEX_CLIENT_DIR=client/dist ANNEX_OPEN_BROWSER=false cargo run -p annex-server
 *
 * Or use the helper script:
 *
 *   bash ../scripts/e2e-server.sh start
 */
// Use system-installed browsers if available (Claude Code environment)
if (process.env.PLAYWRIGHT_BROWSERS_PATH === undefined) {
  const systemPath = '/opt/pw-browsers';
  try {
    const { statSync } = await import('fs');
    if (statSync(systemPath).isDirectory()) {
      process.env.PLAYWRIGHT_BROWSERS_PATH = systemPath;
    }
  } catch { /* not available, use default */ }
}

export default defineConfig({
  testDir: './e2e',
  // Recording a baseline is an explicit act, never a side effect of a run.
  //
  // `'none'` refuses to write a snapshot for any reason — including the
  // first-run "it was missing, so I created it" path, which would let a typo
  // in a surface id silently mint a new baseline that then passes forever.
  // `scripts/ui-audit.sh --update-baselines` passes `--update-snapshots` on
  // the command line, and a CLI flag overrides this, so recording still works
  // exactly where it is meant to.
  updateSnapshots: 'none',
  outputDir: './e2e-results',
  fullyParallel: false, // Run serially — tests share server state
  forbidOnly: !!process.env.CI,
  timeout: 120_000, // ZK proof generation can take 30-60s
  retries: process.env.CI ? 1 : 0,
  workers: 1,
  reporter: [['list'], ['html', { open: 'never', outputFolder: 'e2e-report' }]],

  use: {
    baseURL: 'http://127.0.0.1:3000',
    trace: 'retain-on-failure',
    screenshot: 'on',
    video: 'retain-on-failure',
    // Each test gets a fresh browser context (clean IndexedDB, cookies, etc.)
    storageState: undefined,
  },

  projects: [
    // ── Functional suite ──────────────────────────────────────────────
    // The original assertion suite. Deliberately excludes e2e/audit/ so a
    // plain `npm run test:e2e` keeps its existing meaning and runtime, and
    // so `admin.spec.ts` keeps relying on being the earliest registrant.
    {
      name: 'chromium',
      testIgnore: /audit\//,
      use: { ...devices['Desktop Chrome'] },
    },

    // ── UI audit lane ─────────────────────────────────────────────────
    // `audit-setup` drives the real startup flow once per role and saves
    // storage state (including IndexedDB, which is where the identity, keys
    // and cached membership proof live). `audit` then reuses that state, so
    // each captured surface costs a page load instead of a 30-60s Groth16
    // proof.
    //
    // The audit lane needs a FRESH server: it registers its founder first so
    // `ensure_founder` grants moderator. Run it via `scripts/ui-audit.sh`,
    // which restarts the server before invoking these projects.
    {
      name: 'audit-setup',
      testDir: './e2e/audit',
      testMatch: /roles\.setup\.ts/,
      use: { ...devices['Desktop Chrome'] },
    },
    {
      name: 'audit',
      testDir: './e2e/audit',
      testMatch: /capture\.spec\.ts|manifest\.spec\.ts/,
      dependencies: ['audit-setup'],
      // Captures start from warm storage state, so a surface that is actually
      // reachable lands in seconds. The suite-wide 120s budget exists for cold
      // Groth16 proving and does not apply here — leaving it would mean an
      // unreachable surface burned two minutes per viewport before reporting,
      // which is what makes a broad audit unusable to iterate on.
      timeout: 45_000,
      // Baselines live in a tracked directory, addressed as
      // `<viewport>/<surface>.png`. Deliberately NOT under `e2e-results/`,
      // which Playwright wipes between runs and .gitignore excludes.
      // Update them with `bash scripts/ui-audit.sh --update-baselines`.
      snapshotPathTemplate: 'e2e/audit/baselines/{arg}{ext}',
      use: {
        ...devices['Desktop Chrome'],
        // A call is a surface a user touches, so the audit has to be able to
        // join one. Without a camera and microphone the voice stage can only
        // ever capture pre-call states: `getUserMedia` rejects, the join never
        // completes, and the participant grid, media controls and diagnostics
        // are unreachable. Chromium's fake devices give a deterministic
        // synthetic stream and auto-accept the permission prompt, so the whole
        // in-call journey becomes capturable without a physical device.
        permissions: ['camera', 'microphone'],
        launchOptions: {
          args: [
            '--use-fake-ui-for-media-stream',
            '--use-fake-device-for-media-stream',
            // The fake stream still counts as autoplay; without this the
            // <video> elements stay paused and every tile captures black.
            '--autoplay-policy=no-user-gesture-required',
          ],
        },
        // The runner sets an explicit viewport per capture, and an
        // end-of-test auto-screenshot would pollute the baseline directory.
        screenshot: 'off',
        video: 'off',
        // Tracing snapshots the whole DOM on every action. On a DOM-heavy
        // surface that dominates the run — the device-link QR renders one SVG
        // element per module, and tracing it timed out the test at 45s while
        // the same steps completed in under a second untraced. The audit
        // produces its own diagnostics screenshot and findings ledger, so the
        // trace was redundant as well as expensive.
        trace: 'off',
      },
    },

    // Multi-peer calls. Separate from `audit` because it drives three browser
    // contexts through a real WebRTC session rather than capturing a surface,
    // so it needs minutes rather than seconds and its failures mean something
    // different: not "a screen changed" but "a call cannot be held".
    {
      name: 'group-call',
      testDir: './e2e/audit',
      testMatch: /group-call\.spec\.ts/,
      dependencies: ['audit-setup'],
      timeout: 240_000,
      use: {
        ...devices['Desktop Chrome'],
        permissions: ['camera', 'microphone'],
        launchOptions: {
          args: [
            '--use-fake-ui-for-media-stream',
            '--use-fake-device-for-media-stream',
            '--autoplay-policy=no-user-gesture-required',
          ],
        },
        screenshot: 'off',
        video: 'off',
        trace: 'off',
      },
    },
  ],

  // Don't auto-start the server — we manage it externally via e2e-server.sh
  // This keeps the test runner fast and avoids cargo rebuild on every run.
});
