import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const outputDir = fileURLToPath(new URL('../../e2e-results/voice-unconfigured/', import.meta.url));
export default defineConfig({
  testDir: '.', testMatch: 'voice-unconfigured.live.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 300_000, expect: { timeout: 30_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: { baseURL: process.env.ANNEX_UNCONFIGURED_VOICE_URL, actionTimeout: 30_000,
    trace: 'off', screenshot: 'off', video: 'off' },
  projects: [{ name: 'actual-missing-voice-infrastructure', use: { browserName: 'chromium' } }],
});
