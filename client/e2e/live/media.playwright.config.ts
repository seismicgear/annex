import { defineConfig } from '@playwright/test';
import { existsSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const physical = process.env.ANNEX_MEDIA_SOURCE === 'physical';
const waveform = process.env.ANNEX_MEDIA_AUDIO_WAV
  ? resolve(process.env.ANNEX_MEDIA_AUDIO_WAV)
  : fileURLToPath(new URL('../../../../annex-tts.wav', import.meta.url));
if (!physical && !existsSync(waveform)) {
  throw new Error(`Controlled media acceptance requires an actual speech WAV: ${waveform}`);
}
const mode = physical ? 'physical' : 'controlled';
const outputDir = fileURLToPath(new URL(`../../e2e-results/media-${mode}/`, import.meta.url));

/** Fresh database required. No seeded identities or simulated server responses. */
export default defineConfig({
  testDir: '.',
  testMatch: 'media.live.ts',
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 720_000,
  expect: { timeout: 30_000 },
  outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: {
    actionTimeout: 30_000,
    baseURL: process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000',
    // A real tab is the screen-share source in both modes. Headed Chromium
    // gives the capture picker a real composited surface to capture.
    headless: false,
    screenshot: 'off',
    trace: 'off',
    video: 'off',
    launchOptions: {
      args: [
        '--autoplay-policy=no-user-gesture-required',
        '--auto-accept-camera-and-microphone-capture',
        '--auto-select-tab-capture-source-by-title=Annex controlled screen acceptance',
        '--disable-background-timer-throttling',
        '--disable-backgrounding-occluded-windows',
        '--disable-renderer-backgrounding',
        ...(!physical ? [
          // This Windows GPU stack terminates Chromium's controlled camera
          // outside Annex within one second. Software rendering keeps the
          // native source alive; real RTP and decoder assertions still apply.
          '--disable-gpu',
          '--use-fake-device-for-media-stream',
          `--use-file-for-fake-audio-capture=${waveform}`,
        ] : []),
      ],
    },
  },
  projects: [{ name: `native-media-${mode}`, use: { browserName: 'chromium' } }],
});
