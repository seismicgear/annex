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

    if (role === 'second-member') {
      // One message from somebody else. The founder seeds everything else, so
      // without this every bubble in every capture is the local user's own and
      // `.message` without `.self` — the styling most of a real conversation
      // uses — is never rendered, let alone audited. The role itself already
      // existed for the three-party call in `group-call.spec.ts`; no capture
      // surface had ever used it.
      await selectChannel(page, SEED.defaultChannel);
      await sendMessage(page, SEED.messages.fromOther);
    }

    // `indexedDB: true` is what makes this worth doing: Annex keeps the
    // identity, keys and cached membership proof in IndexedDB, so restoring
    // it skips proof generation entirely on the capture run.
    await context.storageState({ path: storageStatePath(role), indexedDB: true });
    await context.close();
  });
}

/**
 * Re-save each role's state once every identity exists.
 *
 * A cached proof binds to the Merkle root current when it was generated, and
 * each registration moves that root. So after all three roles are created,
 * only the LAST one holds a proof matching the live root — the other two
 * re-prove and re-verify on every page load.
 *
 * Across a hundred-odd captures that is a hundred-odd extra
 * `verify-membership` calls, which trips the server's per-category rate
 * limiter and produces an audit full of rate-limited screenshots that say
 * nothing about the UI.
 *
 * One pass here refreshes each role against the now-final root, so the
 * capture run makes zero verification calls. (This step only works because
 * re-authentication is allowed at all — before that fix it returned 409 and
 * locked the member out.)
 */
setup('refresh warm state against the final Merkle root', async ({ browser }) => {
  for (const role of ROLE_ORDER) {
    const context = await browser.newContext({ storageState: storageStatePath(role) });
    const page = await context.newPage();

    await page.goto('/');
    await expect(
      page.getByRole('button', { name: 'Chat' }),
      `${role} must be able to return to the app`,
    ).toBeVisible({ timeout: 60_000 });
    await expect(page.locator('.channel-list')).toBeVisible({ timeout: 20_000 });

    await context.storageState({ path: storageStatePath(role), indexedDB: true });
    await context.close();
  }
});

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

  // Edit and delete are only offered inside the 60s window after posting, so
  // each action happens immediately after its own send. The three fixtures
  // below are named for states the seeder never actually produced — a message
  // called "This message was edited by the audit seeder" was posted and left
  // alone — so the `(edited)` badge, the edit-history panel, the deleted
  // tombstone and the reply rendering were all unreachable, and
  // `SEED.messages.reply` was dead text. The resulting states persist, so
  // every capture of the message list shows a realistic conversation rather
  // than five untouched bubbles.
  await sendMessage(page, SEED.messages.edited);
  await editMessage(page, SEED.messages.edited, SEED.messages.editedAfter);

  await sendMessage(page, SEED.messages.deleted);
  await deleteMessage(page, SEED.messages.deleted);

  await replyToMessage(page, SEED.messages.replyParent, SEED.messages.reply);
}

/// Edits a message in place. Must be called within the edit window of the
/// send, or the control is not rendered at all.
async function editMessage(
  page: import('@playwright/test').Page,
  original: string,
  replacement: string,
) {
  const bubble = page.locator('.message', { hasText: original.slice(0, 40) }).first();
  await bubble.hover();
  await bubble.locator('.edit-btn').click();
  const input = page.locator('.message-edit-input');
  await expect(input).toBeVisible();
  await input.fill(replacement);
  await input.press('Enter');
  await expect(
    page.locator('.message', { hasText: replacement.slice(0, 40) }).first(),
  ).toBeVisible({ timeout: 15_000 });
  // The badge is the point: without it the edit did not reach the server.
  await expect(
    page.locator('.message', { hasText: replacement.slice(0, 40) }).first()
      .locator('.edited-badge'),
  ).toBeVisible({ timeout: 15_000 });
}

/// Deletes a message. Deletion is a two-click confirm, not a dialog.
async function deleteMessage(page: import('@playwright/test').Page, text: string) {
  const bubble = page.locator('.message', { hasText: text.slice(0, 40) }).first();
  await bubble.hover();
  await bubble.locator('.delete-btn').click();
  await expect(bubble.locator('.delete-btn.confirming')).toBeVisible();
  await bubble.locator('.delete-btn').click();
  await expect(page.locator('.message-deleted-text').first()).toBeVisible({
    timeout: 15_000,
  });
}

/// Replies to a message, producing the `.reply-context` quote block.
async function replyToMessage(
  page: import('@playwright/test').Page,
  parent: string,
  body: string,
) {
  const bubble = page.locator('.message', { hasText: parent.slice(0, 40) }).first();
  await bubble.hover();
  await bubble.locator('.reply-btn').click();
  await expect(page.locator('.reply-bar')).toBeVisible();
  await sendMessage(page, body);
  await expect(page.locator('.reply-context').first()).toBeVisible({ timeout: 15_000 });
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
