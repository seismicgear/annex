import { expect, type Page } from '@playwright/test';

/**
 * Complete the full startup flow: identity creation → server selection → main UI.
 * Handles all possible states (fresh visit, identity exists, server selected).
 */
export async function completeStartup(page: Page) {
  await page.goto('/');

  // Wait for the app to initialize and render something meaningful
  await page.waitForLoadState('domcontentloaded');

  // Phase 1: Identity creation (if on that screen)
  try {
    const createBtn = page.getByRole('button', { name: 'Create New Identity' });
    await createBtn.waitFor({ state: 'visible', timeout: 5000 });
    await createBtn.click();
  } catch {
    // Not on identity screen — already have keys
  }

  // Phase 2 + 3: drive server selection → registration → main UI, recovering
  // from transient startup failures exactly like a real user would.
  //
  // Under a busy/contended server the bootstrap or registration probe can
  // momentarily fail ("Unable to contact server"); the app surfaces this with
  // a Retry button (StartupGate's resetToServerSelection) rather than crashing.
  // A test that only waits for the Chat button would hang on that screen, so we
  // poll: click Continue when on server selection, click Retry on a transient
  // error, and finish when the main UI's Chat button appears. Groth16 proving
  // is CPU-bound (~5-15s typical, longer on slow/contended CI), so we allow a
  // generous overall budget.
  const chatBtn = page.getByRole('button', { name: 'Chat' });
  const continueBtn = page.getByRole('button', { name: 'Continue' });
  const retryBtn = page.getByRole('button', {
    name: /^(Retry|Retry startup|Retry \(cancel running proof\))$/,
  });

  const deadline = Date.now() + 150_000;
  while (Date.now() < deadline) {
    if (await chatBtn.isVisible().catch(() => false)) break;
    if (await continueBtn.first().isVisible().catch(() => false)) {
      await continueBtn.first().click().catch(() => {});
    } else if (await retryBtn.first().isVisible().catch(() => false)) {
      // Transient "Unable to contact server" — recover like a user.
      await retryBtn.first().click().catch(() => {});
    }
    await page.waitForTimeout(1000);
  }
  await expect(chatBtn).toBeVisible({ timeout: 5_000 });
}

/**
 * Join the #General channel and select it.
 */
export async function joinAndSelectGeneral(page: Page) {
  const generalEntry = page.locator('.channel-item', { hasText: 'General' });
  await expect(generalEntry).toBeVisible({ timeout: 5000 });

  // Click the "+" join button if visible
  const joinBtn = generalEntry.locator('.join-btn');
  if (await joinBtn.isVisible({ timeout: 2000 }).catch(() => false)) {
    await joinBtn.click();
    await page.waitForTimeout(1000);
  }

  // Select the channel
  await generalEntry.click();
}
