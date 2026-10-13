import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

// This lane displays bearer invite URLs. Playwright's automatic failure DOM
// snapshot would retain them even with trace/screenshots disabled.
process.env.PLAYWRIGHT_NO_COPY_PROMPT = '1';
const outputDir = fileURLToPath(new URL('../../e2e-results/invite-routing/', import.meta.url));
const spki = process.env.ANNEX_INVITE_TLS_SPKI;
export default defineConfig({
  testDir: '.', testMatch: 'invite-routing.live.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 300_000, expect: { timeout: 25_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: {
    baseURL: process.env.ANNEX_LIVE_URL ?? 'https://annex-invite-check.test:3478',
    actionTimeout: 30_000, navigationTimeout: 30_000,
    trace: 'off', screenshot: 'off', video: 'off', ignoreHTTPSErrors: false,
    launchOptions: { args: [
      '--host-resolver-rules=MAP annex-invite-check.test 127.0.0.1',
      '--no-proxy-server',
      ...(spki ? [`--ignore-certificate-errors-spki-list=${spki}`] : []),
    ] },
  },
  projects: [{ name: 'actual-hosted-invite-routing', use: { browserName: 'chromium' } }],
});
