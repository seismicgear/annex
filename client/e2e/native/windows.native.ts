import { chromium, expect, test } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';

test('actual installed Windows Host preserves its founder and message across upgrade and recovery', async () => {
  const phase = process.env.ANNEX_NATIVE_PHASE;
  const output = process.env.ANNEX_NATIVE_EVIDENCE;
  if (!['baseline', 'upgrade', 'recovery'].includes(phase ?? '') || !output) throw new Error('Explicit native acceptance phase and evidence directory required');
  const endpoint = process.env.ANNEX_NATIVE_CDP;
  if (!/^http:\/\/127\.0\.0\.1:\d+$/.test(endpoint ?? '')) throw new Error('Expected isolated loopback WebView endpoint');
  const browser = await chromium.connectOverCDP(endpoint!);
  let observedPage: import('@playwright/test').Page | undefined;
  try {
    const pages = browser.contexts().flatMap(context => context.pages());
    expect(pages).toHaveLength(1);
    const page = pages[0];
    observedPage = page;
    // The native debug endpoint is ready before WebView2 navigates away from
    // about:blank. Its already-complete DOM is not application readiness.
    await expect(page).toHaveTitle('Annex', { timeout: 90_000 });
    // No injected application state, direct command invocation or seeded keys.
    const host = page.getByRole('button', { name: 'Start Hosting', exact: true });
    await expect.poll(async () => await host.isVisible() || await page.getByRole('button', { name: 'Admin', exact: true }).isVisible(), { timeout: 90_000 }).toBe(true);
    if (await host.isVisible()) await host.click();
    await expect(page.getByRole('button', { name: 'Admin', exact: true })).toBeVisible({ timeout: 180_000 });
    await page.getByRole('button', { name: 'Identity', exact: true }).click();
    await expect(page.getByText('Server founder', { exact: true })).toBeVisible();
    await page.getByRole('dialog').getByRole('button', { name: 'Close', exact: true }).click();
    await page.getByRole('button', { name: 'Chat', exact: true }).click();
    await page.locator('.channel-item').filter({ hasText: 'General' }).first().click();
    const marker = 'Annex native installation upgrade persistence acceptance';
    if (phase === 'baseline') {
      await page.getByPlaceholder('Type a message...').fill(marker);
      await page.getByRole('button', { name: 'Send', exact: true }).click();
    }
    await expect(page.locator('.message-content').filter({ hasText: marker })).toHaveCount(1);
    await expect(page.locator('.ws-indicator')).toHaveClass(/\bconnected\b/);
    await page.screenshot({ path: join(output, `${phase}-native-window.png`) });
    await writeFile(join(output, `${phase}-ui.json`), JSON.stringify({
      phase, nativeWebView: true, realHostStartup: true, founderBadge: true,
      storedMessageVisible: true, websocketConnected: true,
      mocks: false, inAppUpdaterTested: false,
    }, null, 2) + '\n');
  } catch (error) {
    if (observedPage && !observedPage.isClosed()) {
      await observedPage.screenshot({ path: join(output, `${phase}-failure.png`) }).catch(() => {});
      await writeFile(join(output, `${phase}-failure.json`), JSON.stringify({
        url: observedPage.url(), title: await observedPage.title().catch(() => null),
        error: String(error),
      }, null, 2) + '\n');
    }
    throw error;
  } finally {
    // Disconnect the observer; the owning workflow controls the actual process.
    await browser.close();
  }
});
