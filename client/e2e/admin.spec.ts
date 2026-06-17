/**
 * Admin & channel management — drives the founder/moderator surfaces that the
 * happy-path chat suite never touches: the Admin panel (Server Settings, Server
 * Policy, Member Management, Channel Management) and the full create → use →
 * delete lifecycle of a channel, clicking through the real UI controls a user
 * would use.
 *
 * This file sorts first alphabetically, so on the shared fresh e2e DB its
 * identity is the EARLIEST registrant — which the server's `ensure_founder`
 * path promotes to moderator. That gives it the admin menu + create/delete
 * rights. It only creates and deletes its OWN uniquely-named channel, so it
 * never disturbs #General or the other specs.
 */
import { test, expect } from '@playwright/test';
import { completeStartup } from './helpers';

test.describe('Admin & channel management (founder)', () => {
  test('admin panel sections + channel create/use/delete', async ({ page }) => {
    test.setTimeout(180_000);

    await completeStartup(page);
    await expect(page.locator('.app-layout')).toBeVisible();

    // The moderator-only admin (gear) button must be present for the founder.
    const adminBtn = page.locator('.admin-menu-btn');
    await expect(adminBtn).toBeVisible({ timeout: 30_000 });

    // Open each admin section through the dropdown and prove it renders.
    const sections = [
      { item: 'Server Settings', heading: 'Server Settings', shot: 'admin-server' },
      { item: 'Server Policy', heading: 'Server Policy', shot: 'admin-policy' },
      { item: 'Member Management', heading: 'Member Management', shot: 'admin-members' },
      { item: 'Channel Management', heading: 'Channel Management', shot: 'admin-channels' },
    ];
    for (const s of sections) {
      await adminBtn.click();
      await page.getByRole('button', { name: s.item }).click();
      await expect(page.getByRole('heading', { name: s.heading }).first()).toBeVisible({ timeout: 15_000 });

      if (s.item === 'Server Settings') {
        // The Public URL must auto-populate from the request — the server now
        // derives it instead of leaving the operator with a blank field.
        const urlInput = page.getByPlaceholder('https://your-server.example.com');
        await expect(urlInput).toBeVisible();
        await expect(urlInput).not.toHaveValue('');

        // Point it at a real public address and confirm the shareable invite
        // link is generated AND routes through the marketing site
        // (monolithannex.com) — the end-to-end invite-router deliverable.
        await urlInput.fill('https://annex.demo.example');
        await page.getByRole('button', { name: 'Save', exact: true }).click();
        const inviteLink = page.locator('.share-link-input');
        await expect(inviteLink).toBeVisible({ timeout: 15_000 });
        await expect(inviteLink).toHaveValue(/^https:\/\/monolithannex\.com\/invite\//, {
          timeout: 15_000,
        });
      }

      await page.screenshot({ path: `e2e-results/${s.shot}.png`, fullPage: true });
    }

    // Back to chat to use the channel sidebar.
    await page.getByRole('button', { name: 'Chat' }).click();
    await expect(page.locator('.app-layout')).toBeVisible();

    // ── Create a channel via the "+" control, like a user ──
    const channelName = `e2e-admin-${Date.now()}`;
    await page.locator('.create-channel-btn').click();
    await expect(page.getByRole('heading', { name: 'Create Channel' })).toBeVisible({ timeout: 10_000 });
    await page.getByPlaceholder('general').fill(channelName);
    await page.getByRole('button', { name: 'Create' }).click();

    const newChannel = page.locator('.channel-item', { hasText: channelName });
    await expect(newChannel).toBeVisible({ timeout: 15_000 });
    await page.screenshot({ path: 'e2e-results/admin-channel-created.png', fullPage: true });

    // Select it and post a message so we know the created channel is usable.
    await newChannel.click();
    const input = page.getByPlaceholder('Type a message...');
    await expect(input).toBeVisible({ timeout: 10_000 });
    const message = `hello in ${channelName}`;
    await input.fill(message);
    await page.getByRole('button', { name: 'Send' }).click();
    await expect(page.getByText(message).first()).toBeVisible({ timeout: 15_000 });

    // ── Delete the channel via Admin → Channel Management ──
    // The delete uses window.confirm(); accept it the way a user clicking "OK" would.
    page.on('dialog', (d) => void d.accept());
    await adminBtn.click();
    await page.getByRole('button', { name: 'Channel Management' }).click();
    const row = page.locator('.channel-manager-item', { hasText: channelName });
    await expect(row).toBeVisible({ timeout: 15_000 });
    await row.getByRole('button', { name: 'Delete' }).click();

    // It disappears from the manager and from the chat sidebar.
    await expect(page.locator('.channel-manager-item', { hasText: channelName })).toHaveCount(0, { timeout: 15_000 });
    await page.screenshot({ path: 'e2e-results/admin-channel-deleted.png', fullPage: true });

    await page.getByRole('button', { name: 'Chat' }).click();
    await expect(page.locator('.channel-item', { hasText: channelName })).toHaveCount(0, { timeout: 10_000 });
  });
});
