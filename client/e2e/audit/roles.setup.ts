/**
 * Setup project for the audit lane.
 *
 * Runs before `capture.spec.ts` (wired via `dependencies` in
 * `playwright.config.ts`) and does two things, in this order:
 *
 * 1. Creates each role's identity against a freshly-provisioned server and
 *    saves `storageState({ indexedDB: true })` so the capture run starts warm.
 * 2. Seeds fixture data as the founder, so panels are not empty when captured.
 *
 * Both steps drive the real UI rather than writing to SQLite or forging API
 * calls. That is slower, but it means the seed exercises exactly the paths a
 * user does — if channel creation regresses, seeding fails loudly here instead
 * of producing a green audit over a broken app.
 *
 * `test.describe.configure({ mode: 'serial' })` is load-bearing: the server
 * promotes the earliest registrant to moderator (`ensure_founder`), so the
 * founder identity must reach the server before any other.
 */

import { mkdirSync } from 'node:fs';
import { expect, test as setup } from '@playwright/test';
import { completeStartup } from '../helpers';
import { AUTH_DIR, ROLE_ORDER, SEED, storageStatePath } from './roles';
import { selectChannel } from './nav';

setup.describe.configure({ mode: 'serial' });

// A cold context pays a real Groth16 proof; on a contended 4-core box that can
// approach a minute, and the founder run also seeds.
setup.setTimeout(300_000);

/** Drive the full startup flow in a clean context and return it. */
async function warmRole(browser: import('@playwright/test').Browser) {
  const context = await browser.newContext();
  const page = await context.newPage();

  await completeStartup(page);
  await expect(page.locator('.channel-list')).toBeVisible({ timeout: 30_000 });

  return { context, page };
}

for (const role of ROLE_ORDER) {
  setup(`warm auth state: ${role}`, async ({ browser }) => {
    mkdirSync(AUTH_DIR, { recursive: true });

    const { context, page } = await warmRole(browser);

    if (role === 'founder') {
      // The gear only renders when the server reports `can_moderate`. If this
      // fails, the founder was not the earliest registrant — which means some
      // other test or a stale DB reached the server first, and every admin
      // surface in the manifest would be uncapturable.
      await expect(
        page.locator('.admin-menu-btn'),
        'founder must be the earliest registrant so ensure_founder grants moderator',
      ).toBeVisible({ timeout: 30_000 });

      await seedFixtures(page);
    }

    // `indexedDB: true` is what makes this worth doing: Annex keeps the
    // identity, keys and cached membership proof in IndexedDB, so restoring
    // it skips proof generation entirely on the capture run.
    await context.storageState({ path: storageStatePath(role), indexedDB: true });
    await context.close();
  });
}

/**
 * Create the channels and messages the manifest expects to find.
 *
 * Covers all five channel types plus a deliberately empty channel, and a
 * message of each kind the message list can render.
 */
async function seedFixtures(page: import('@playwright/test').Page) {
  const channels: [string, string][] = [
    [SEED.channels.text, 'Text'],
    [SEED.channels.voice, 'Voice'],
    [SEED.channels.hybrid, 'Hybrid'],
    [SEED.channels.agent, 'Agent'],
    [SEED.channels.broadcast, 'Broadcast'],
    [SEED.emptyChannel, 'Text'],
  ];

  for (const [name, type] of channels) {
    await page.locator('.create-channel-btn').click();
    const dialog = page.locator('.dialog');
    await expect(dialog).toBeVisible();
    await dialog.getByPlaceholder('general').fill(name);
    await dialog.locator('select').selectOption(type);
    await dialog.getByRole('button', { name: 'Create' }).click();
    await expect(dialog).toBeHidden({ timeout: 20_000 });
    await expect(page.locator('.channel-item', { hasText: name })).toBeVisible({
      timeout: 15_000,
    });
  }

  // Messages go in the default channel so the busiest capture (the chat view)
  // shows a realistic conversation rather than one lonely bubble.
  await selectChannel(page, SEED.defaultChannel);

  await sendMessage(page, SEED.messages.plain);
  await sendMessage(page, SEED.messages.long);
  await sendMessage(page, SEED.messages.replyParent);
  await sendMessage(page, SEED.messages.edited);
  await sendMessage(page, SEED.messages.deleted);
}

async function sendMessage(page: import('@playwright/test').Page, text: string) {
  const composer = page.getByPlaceholder('Type a message...');
  await composer.fill(text);
  await page.getByRole('button', { name: 'Send' }).click();
  await expect(page.locator('.message', { hasText: text.slice(0, 40) }).first()).toBeVisible({
    timeout: 20_000,
  });
  // A failed send renders as `.message.failed`; catching it here keeps a
  // broken WS path from silently producing an empty-looking audit.
  await expect(page.locator('.message.failed')).toHaveCount(0);
}
