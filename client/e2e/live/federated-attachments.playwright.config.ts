import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const outputDir = fileURLToPath(new URL('../../e2e-results/federated-attachments/', import.meta.url));
export default defineConfig({
  testDir: '.', testMatch: 'federated-attachments.live.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 600_000, expect: { timeout: 30_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  // Upload grants and identity headers stay in memory; neither a trace nor a
  // screenshot is necessary to prove downloaded bytes and storage cleanup.
  use: { actionTimeout: 30_000, trace: 'off', screenshot: 'off', video: 'off' },
  projects: [{ name: 'actual-ordinary-federated-attachments', use: { browserName: 'chromium' } }],
});
