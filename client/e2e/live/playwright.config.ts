import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const outputDir = fileURLToPath(new URL('../../e2e-results/live/', import.meta.url));
const reportDir = fileURLToPath(new URL('../../e2e-report/live/', import.meta.url));

/** Run only against a dedicated fresh database; no other browser lane may share it. */
export default defineConfig({
  testDir: '.',
  testMatch: 'session.live.ts',
  fullyParallel: false,
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 240_000,
  expect: { timeout: 20_000 },
  outputDir,
  reporter: [
    ['list', { printSteps: true }],
    ['json', { outputFile: `${outputDir}/results.json` }],
    ['html', { outputFolder: reportDir, open: 'never' }],
  ],
  use: {
    actionTimeout: 30_000,
    baseURL: process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000',
    // The link flow displays a key transfer and its pairing code. Evidence is
    // captured explicitly AFTER closing it; traces/video would retain secrets.
    screenshot: 'off',
    trace: 'off',
    video: 'off',
  },
  projects: [{ name: 'live-session', use: { browserName: 'chromium' } }],
});
