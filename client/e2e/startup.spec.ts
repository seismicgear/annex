import { test, expect } from '@playwright/test';

test.describe('Startup Flow', () => {
  test('shows identity creation screen on first visit', async ({ page }) => {
    await page.goto('/');
    await expect(page.getByRole('heading', { name: 'Create Your Identity' })).toBeVisible();
    await expect(page.getByText('Ready to create identity')).toBeVisible();
    await expect(page.getByRole('button', { name: 'Create New Identity' })).toBeVisible();
    await expect(page.getByRole('button', { name: 'Link from Another Device' })).toBeVisible();
    await expect(page.getByText('Import Backup')).toBeVisible();
  });

  test('shows ANNEX header branding', async ({ page }) => {
    await page.goto('/');
    const header = page.locator('header');
    await expect(header).toBeVisible();
    await expect(header.getByText('Annex')).toBeVisible();
  });

  test('creates identity and advances to server selection', async ({ page }) => {
    await page.goto('/');
    await page.getByRole('button', { name: 'Create New Identity' }).click();
    await expect(page.getByText('Use This Server')).toBeVisible({ timeout: 30000 });
    await expect(page.getByRole('button', { name: 'Continue' })).toBeVisible();
    await expect(page.getByText(/Connect to.*Server/)).toBeVisible();
  });

  test('health API returns ok', async ({ page }) => {
    await page.goto('/');
    const result = await page.evaluate(async () => {
      const resp = await fetch('/health');
      return { ok: resp.ok, body: await resp.json() };
    });
    expect(result.ok).toBe(true);
    expect(result.body.status).toBe('ok');
  });
});
