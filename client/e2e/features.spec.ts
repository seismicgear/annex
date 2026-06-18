/**
 * Feature evidence — exercises message edit, message delete, reply, and the
 * top-level view navigation against the real server, capturing a screenshot
 * for each. These paths work for any authenticated member (no founder
 * capability required), so they run reliably regardless of test order.
 */
import { test, expect } from '@playwright/test';
import { completeStartup, joinAndSelectGeneral } from './helpers';

async function sendMessage(page: import('@playwright/test').Page, text: string) {
  const input = page.getByPlaceholder('Type a message...');
  await expect(input).toBeVisible({ timeout: 10_000 });
  await input.fill(text);
  await page.getByRole('button', { name: 'Send' }).click();
  const msg = page.locator('.message', { hasText: text }).first();
  await expect(msg).toBeVisible({ timeout: 15_000 });
  // Never accept a message that the server rejected.
  await expect(page.locator('.message.failed', { hasText: text })).toHaveCount(0);
  return msg;
}

test.describe('Feature evidence', () => {
  test('navigate Chat / Federation / Events without page errors', async ({ page }) => {
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));

    await completeStartup(page);
    await expect(page.locator('.app-layout')).toBeVisible();

    await page.getByRole('button', { name: 'Federation' }).click();
    await expect(page.locator('.view-content')).toBeVisible();
    await page.screenshot({ path: 'e2e-results/feature-federation.png', fullPage: true });

    await page.getByRole('button', { name: 'Events' }).click();
    await expect(page.locator('.view-content')).toBeVisible();
    await page.screenshot({ path: 'e2e-results/feature-events.png', fullPage: true });

    await page.getByRole('button', { name: 'Chat' }).click();
    await expect(page.locator('.app-layout')).toBeVisible();

    expect(errors, `uncaught page errors: ${errors.join(' | ')}`).toEqual([]);
  });

  test('edit own message', async ({ page }) => {
    await completeStartup(page);
    await joinAndSelectGeneral(page);

    const original = `edit-me-${Date.now()}`;
    const msg = await sendMessage(page, original);

    await msg.hover();
    await msg.locator('.edit-btn').click();
    const editInput = msg.locator('.message-edit-input');
    await expect(editInput).toBeVisible();
    const edited = `${original}-EDITED`;
    await editInput.fill(edited);
    await msg.getByRole('button', { name: 'Save' }).click();

    await expect(page.locator('.message', { hasText: edited })).toBeVisible({ timeout: 15_000 });
    await expect(page.locator('.edited-badge').first()).toBeVisible({ timeout: 10_000 });
    await page.screenshot({ path: 'e2e-results/feature-edit.png', fullPage: true });
  });

  test('delete own message', async ({ page }) => {
    await completeStartup(page);
    await joinAndSelectGeneral(page);

    const text = `delete-me-${Date.now()}`;
    const msg = await sendMessage(page, text);

    await msg.hover();
    await msg.locator('.delete-btn').click(); // first click → "Confirm?"
    await msg.hover();
    await msg.locator('.delete-btn.confirming').click(); // second click → delete

    // `.first()` mirrors the edited-badge / reply-context assertions above:
    // the shared #General channel can already hold deleted messages from
    // earlier tests, so match the first rather than violating strict mode.
    await expect(page.locator('.message-deleted-text').first()).toBeVisible({ timeout: 15_000 });
    await page.screenshot({ path: 'e2e-results/feature-delete.png', fullPage: true });
  });

  test('reply to a message', async ({ page }) => {
    await completeStartup(page);
    await joinAndSelectGeneral(page);

    const target = `reply-target-${Date.now()}`;
    const msg = await sendMessage(page, target);

    await msg.hover();
    await msg.locator('.reply-btn').click();
    await expect(page.locator('.reply-bar')).toBeVisible({ timeout: 10_000 });

    const replyText = `the-reply-${Date.now()}`;
    await sendMessage(page, replyText);
    // The reply renders with a reply-context referencing the target.
    await expect(page.locator('.reply-context').first()).toBeVisible({ timeout: 10_000 });
    await page.screenshot({ path: 'e2e-results/feature-reply.png', fullPage: true });
  });
});
