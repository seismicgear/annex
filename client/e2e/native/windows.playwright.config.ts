import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.', testMatch: 'windows.native.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 300_000, expect: { timeout: 30_000 },
  outputDir: process.env.ANNEX_NATIVE_EVIDENCE,
  reporter: [['list']],
  use: { trace: 'off', screenshot: 'off', video: 'off' },
});
