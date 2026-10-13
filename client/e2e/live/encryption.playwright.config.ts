import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const outputDir = fileURLToPath(new URL('../../e2e-results/encryption/', import.meta.url));
export default defineConfig({
  testDir: '.', testMatch: 'encryption.live.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 600_000, expect: { timeout: 25_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: {
    baseURL: process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000',
    actionTimeout: 30_000,
    // Device linking displays an identity transfer and pairing secret.
    trace: 'off', screenshot: 'off', video: 'off',
  },
  projects: [{ name: 'live-encryption', use: { browserName: 'chromium' } }],
});
