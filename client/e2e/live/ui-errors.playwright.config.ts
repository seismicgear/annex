import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const outputDir = fileURLToPath(new URL('../../e2e-results/ui-errors/', import.meta.url));

/** Fresh real server required. Browser permissions and transport are changed;
 * API responses, media devices, application stores and server state are not faked. */
export default defineConfig({
  testDir: '.', testMatch: 'ui-errors.live.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 300_000, expect: { timeout: 30_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: {
    baseURL: process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000',
    actionTimeout: 30_000, headless: false, screenshot: 'off', trace: 'off', video: 'off',
    launchOptions: { args: ['--autoplay-policy=no-user-gesture-required', '--disable-background-timer-throttling'] },
  },
  projects: [{ name: 'actual-browser-errors', use: { browserName: 'chromium' } }],
});
