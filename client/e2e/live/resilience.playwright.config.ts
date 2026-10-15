import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const outputDir = fileURLToPath(new URL('../../e2e-results/resilience/', import.meta.url));
export default defineConfig({
  testDir: '.', testMatch: 'resilience.live.ts', fullyParallel: false,
  workers: 1, retries: 0, forbidOnly: true, timeout: 300_000,
  expect: { timeout: 25_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: {
    actionTimeout: 30_000,
    baseURL: process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000',
    trace: 'off', screenshot: 'off', video: 'off',
  },
  projects: [{ name: 'live-resilience', use: { browserName: 'chromium' } }],
});
