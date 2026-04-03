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
    await page.goto('/');

    // Create identity
    await page.getByRole('button', { name: 'Create New Identity' }).click();

    // Select server
    await expect(page.getByRole('button', { name: 'Continue' })).toBeVisible({ timeout: 30000 });
    await page.getByRole('button', { name: 'Continue' }).click();

    // Wait for ZK proof + registration
    await expect(page.getByRole('button', { name: 'Chat' })).toBeVisible({ timeout: 90000 });

    // Verify main UI
    await expect(page.locator('.sidebar-left')).toBeVisible();
    await expect(page.locator('.chat-area')).toBeVisible();
    await expect(page.getByText('General')).toBeVisible({ timeout: 5000 });
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
    await input.fill('Hello from Playwright E2E test!');
    await page.getByRole('button', { name: 'Send' }).click();

    await expect(page.getByText('Hello from Playwright E2E test!')).toBeVisible({ timeout: 15000 });
  });
});
