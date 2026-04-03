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
    {
      name: 'chromium',
      use: { ...devices['Desktop Chrome'] },
    },
  ],

  // Don't auto-start the server — we manage it externally via e2e-server.sh
  // This keeps the test runner fast and avoids cargo rebuild on every run.
});
