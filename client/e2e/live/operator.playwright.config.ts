import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const outputDir = fileURLToPath(new URL('../../e2e-results/operator/', import.meta.url));

export default defineConfig({
  testDir: '.', testMatch: 'operator.live.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 360_000, expect: { timeout: 25_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: {
    actionTimeout: 30_000,
    baseURL: process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000',
    // Password admission and recovery shards must not enter recordings.
    trace: 'off', screenshot: 'off', video: 'off',
  },
  projects: [{ name: 'live-operator', use: { browserName: 'chromium' } }],
});
