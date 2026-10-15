import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';
const outputDir = fileURLToPath(new URL('../../e2e-results/federation-authority/', import.meta.url));
export default defineConfig({
  testDir: '.', testMatch: 'federation-authority.live.ts', workers: 1, retries: 0, forbidOnly: true,
  timeout: 600_000, expect: { timeout: 35_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: { actionTimeout: 30_000, trace: 'off', screenshot: 'off', video: 'off' },
  projects: [{ name: 'actual-federation-authority', use: { browserName: 'chromium' } }],
});
