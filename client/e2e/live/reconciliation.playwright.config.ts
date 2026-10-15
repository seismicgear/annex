import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';
const outputDir = fileURLToPath(new URL('../../e2e-results/reconciliation/', import.meta.url));
export default defineConfig({
  testDir: '.', testMatch: 'reconciliation.live.ts', workers: 1, retries: 0, forbidOnly: true,
  timeout: 660_000, expect: { timeout: 30_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: { actionTimeout: 30_000, trace: 'off', screenshot: 'off', video: 'off' },
  projects: [{ name: 'actual-populated-reconciliation', use: { browserName: 'chromium' } }],
});
