import { defineConfig } from '@playwright/test';
import { fileURLToPath } from 'node:url';

const outputDir = fileURLToPath(new URL('../../e2e-results/encrypted-attachments/', import.meta.url));
// Keep the root's captured backend session intact. All test actors use a fresh
// forwarding origin so Chromium Blob uploads can be observed on the real wire.
process.env.ANNEX_ATTACHMENT_BACKEND_URL ??= process.env.ANNEX_LIVE_URL ?? 'http://127.0.0.1:3000';
const proxyPort = Number(process.env.ANNEX_ATTACHMENT_PROXY_PORT ?? '13022');
if (!Number.isInteger(proxyPort) || proxyPort < 1024 || proxyPort > 65535) throw new Error('Invalid attachment proxy port');
process.env.ANNEX_LIVE_URL = `http://127.0.0.1:${proxyPort}`;
export default defineConfig({
  testDir: '.', testMatch: 'encrypted-attachments.live.ts', workers: 1, retries: 0,
  forbidOnly: true, timeout: 900_000, expect: { timeout: 30_000 }, outputDir,
  reporter: [['list', { printSteps: true }], ['json', { outputFile: `${outputDir}/results.json` }]],
  use: {
    baseURL: process.env.ANNEX_LIVE_URL, actionTimeout: 30_000,
    // Linking displays private transfer material. Only bounded public receipts
    // and hashes are attached by the tests themselves.
    trace: 'off', screenshot: 'off', video: 'off',
  },
  projects: [{ name: 'live-encrypted-attachments', use: { browserName: 'chromium' } }],
});
