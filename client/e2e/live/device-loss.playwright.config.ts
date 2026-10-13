import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const outputDir = fileURLToPath(new URL('../../e2e-results/device-loss/', import.meta.url));
export default defineConfig({
  testDir: '.', testMatch: 'device-loss.live.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 540_000, expect: { timeout: 30_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: { baseURL: process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000', actionTimeout: 30_000,
    headless: false, trace: 'off', screenshot: 'off', video: 'off',
    launchOptions: { args: ['--disable-gpu', '--use-fake-device-for-media-stream',
      '--auto-accept-camera-and-microphone-capture', '--autoplay-policy=no-user-gesture-required'] } },
  projects: [{ name: 'actual-browser-device-availability-transition', use: { browserName: 'chromium' } }],
});
