import { defineConfig } from '@playwright/test';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const waveform = fileURLToPath(new URL('../../../../annex-tts.wav', import.meta.url));
if (!existsSync(waveform)) throw new Error('Actual speech waveform is required');
const outputDir = fileURLToPath(new URL('../../e2e-results/relay-media/', import.meta.url));
export default defineConfig({
  testDir: '.', testMatch: 'relay-media.live.ts', workers: 1, retries: 0, forbidOnly: true,
  timeout: 720_000, expect: { timeout: 35_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: { baseURL: process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3065', actionTimeout: 30_000,
    headless: false, trace: 'off', screenshot: 'off', video: 'off', launchOptions: { args: [
      '--disable-gpu', '--use-fake-device-for-media-stream', `--use-file-for-fake-audio-capture=${waveform}`,
      '--auto-accept-camera-and-microphone-capture', '--autoplay-policy=no-user-gesture-required',
      '--auto-select-tab-capture-source-by-title=Annex TURN screen acceptance',
      '--disable-background-timer-throttling', '--disable-backgrounding-occluded-windows', '--disable-renderer-backgrounding',
    ] } },
  projects: [{ name: 'actual-coturn-relayed-media', use: { browserName: 'chromium' } }],
});
