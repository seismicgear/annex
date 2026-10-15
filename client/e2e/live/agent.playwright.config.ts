import { defineConfig } from '@playwright/test';
import { existsSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

const waveform = process.env.ANNEX_MEDIA_AUDIO_WAV
  ? resolve(process.env.ANNEX_MEDIA_AUDIO_WAV)
  : fileURLToPath(new URL('../../../../annex-tts.wav', import.meta.url));
if (!existsSync(waveform)) throw new Error('Agent acceptance requires the real Piper speech WAV');
const outputDir = fileURLToPath(new URL('../../e2e-results/agent/', import.meta.url));
export default defineConfig({
  testDir: '.', testMatch: 'agent.live.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 900_000, expect: { timeout: 30_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: {
    actionTimeout: 30_000,
    baseURL: process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000',
    trace: 'off', video: 'off', screenshot: 'off',
    launchOptions: { args: [
      '--autoplay-policy=no-user-gesture-required', '--auto-accept-camera-and-microphone-capture',
      '--use-fake-device-for-media-stream', `--use-file-for-fake-audio-capture=${waveform}`,
    ] },
  },
  projects: [{ name: 'real-model-agent', use: { browserName: 'chromium' } }],
});
