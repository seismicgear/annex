import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.', testMatch: 'windows-updater.native.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 600_000, expect: { timeout: 30_000 },
  outputDir: process.env.ANNEX_NATIVE_EVIDENCE,
  reporter: [['list']],
  use: { trace: 'off', screenshot: 'off', video: 'off' },
});
