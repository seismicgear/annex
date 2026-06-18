/**
 * Full-flow E2E tests that go through identity creation, ZK proof generation,
 * and the main chat UI. These are heavier tests that exercise the complete
 * user journey including WebAssembly proof computation.
 *
 * Run separately: npx playwright test e2e/full-flow.spec.ts
 */
import { test, expect } from '@playwright/test';
import { completeStartup, joinAndSelectGeneral } from './helpers';

test.describe('Full Flow', () => {
  test('identity → server → main chat UI', async ({ page }) => {
    // completeStartup performs the same journey (create identity → choose
    // server → ZK proof → main UI) but recovers from transient startup errors.
    await completeStartup(page);

    // Verify main UI
    await expect(page.locator('.sidebar-left')).toBeVisible();
    await expect(page.locator('.chat-area')).toBeVisible();
    await expect(page.getByText('General').first()).toBeVisible({ timeout: 5000 });
  });

  test('can navigate tabs after startup', async ({ page }) => {
    await completeStartup(page);

    await expect(page.locator('.app-layout')).toBeVisible();

    await page.getByRole('button', { name: 'Federation' }).click();
    await expect(page.locator('.view-content')).toBeVisible();

    await page.getByRole('button', { name: 'Events' }).click();
    await expect(page.locator('.view-content')).toBeVisible();

    await page.getByRole('button', { name: 'Chat' }).click();
    await expect(page.locator('.app-layout')).toBeVisible();
  });

  test('can join channel and send message', async ({ page }) => {
    await completeStartup(page);
    await joinAndSelectGeneral(page);

    const input = page.getByPlaceholder('Type a message...');
    await expect(input).toBeVisible({ timeout: 5000 });
    // Unique per run so re-running against a persistent #General never collides
    // (a fixed string resolves to multiple matches → strict-mode violation).
    const message = `Hello from Playwright E2E test ${Date.now()}`;
    await input.fill(message);
    await page.getByRole('button', { name: 'Send' }).click();

    await expect(page.getByText(message).first()).toBeVisible({ timeout: 15000 });
  });
});
