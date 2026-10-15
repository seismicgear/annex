import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const outputDir = fileURLToPath(new URL('../../e2e-results/navigation/', import.meta.url));
export default defineConfig({
  testDir: '.', testMatch: 'navigation.live.ts', fullyParallel: false,
  workers: 1, retries: 0, forbidOnly: true, timeout: 300_000,
  expect: { timeout: 25_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: {
    baseURL: process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000', actionTimeout: 30_000,
    // Backups contain private key material. Never put their content in a trace,
    // screenshot, video, attachment, or failed string-equality assertion.
    trace: 'off', screenshot: 'off', video: 'off',
  },
  projects: [{ name: 'live-navigation', use: { browserName: 'chromium' } }],
});
