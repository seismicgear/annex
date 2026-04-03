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

  // Phase 2: Server selection (wait for Continue button or Chat button)
  // The app may go directly to the registration/proof phase if auto-connecting
  const continueOrChat = page.getByRole('button', { name: /^(Continue|Chat)$/ });
  await expect(continueOrChat).toBeVisible({ timeout: 30000 });

  // If we see Continue, click it to proceed
  const continueBtn = page.getByRole('button', { name: 'Continue' });
  if (await continueBtn.isVisible().catch(() => false)) {
    await continueBtn.click();
  }

  // Phase 3: Wait for main UI (after ZK proof generation)
  await expect(page.getByRole('button', { name: 'Chat' })).toBeVisible({ timeout: 90000 });
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
