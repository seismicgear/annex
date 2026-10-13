import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const outputDir = fileURLToPath(new URL('../../e2e-results/history-invariant/', import.meta.url));
export default defineConfig({
  testDir: '.', testMatch: 'history-invariant.live.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 300_000, expect: { timeout: 30_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: { baseURL: process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000', actionTimeout: 30_000,
    trace: 'off', screenshot: 'off', video: 'off' },
  projects: [{ name: 'actual-edit-history-invariant', use: { browserName: 'chromium' } }],
});
