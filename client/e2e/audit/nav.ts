/**
 * Navigation and capture-stabilisation helpers for the UI audit harness.
 *
 * Two jobs:
 *
 * 1. **Determinism.** Every run generates fresh cryptographic identities, so
 *    pseudonyms, Merkle leaf indices, invite codes and timestamps differ each
 *    time and are rendered on screen. Rather than trying to make identities
 *    reproducible (which would mean faking the thing under test), the harness
 *    masks those regions at capture time and disables animation.
 *
 * 2. **Navigation.** Annex has no router — every view is Zustand/`useState`
 *    driven — so "go to the admin policy page" is a click path, not a URL.
 *    These helpers name those click paths once so the manifest stays
 *    declarative.
 */

import { expect, type Locator, type Page } from '@playwright/test';
import type { AppRole } from './roles';

/**
 * Regions whose content is identity- or time-derived and therefore differs on
 * every run. Masked on every screenshot so committed baselines stay stable.
 *
 * Keep this list tight: masking hides real regressions, so a selector belongs
 * here only if its content genuinely cannot be stable across runs.
 */
export const NONDETERMINISTIC_SELECTORS = [
  '.pseudonym', // StatusBar + IdentitySetup: truncated pseudonym id
  '.persona-context-name', // header persona chip
  '.message-timestamp',
  '.event-time', // EventLog "Time" column
  '.share-link-input', // invite links embed a random code
  '.identity-commitment', // IdentitySettings cryptographic id block
  '.member-pseudonym', // admin member rows
  '.agent-pseudonym',
  '[data-nondeterministic]', // opt-in escape hatch added in components
];

/**
 * CSS injected before every capture.
 *
 * Disables animation/transition globally and hides the caret. Without this,
 * spinners and the reconnection banner's transition land mid-frame and every
 * baseline diffs against itself.
 */
const CAPTURE_STYLESHEET = `
  *, *::before, *::after {
    animation-duration: 0s !important;
    animation-delay: 0s !important;
    animation-iteration-count: 1 !important;
    transition-duration: 0s !important;
    transition-delay: 0s !important;
    caret-color: transparent !important;
    scroll-behavior: auto !important;
  }
`;

/** Apply capture-time stabilisation. Safe to call more than once per page. */
export async function stabilize(page: Page): Promise<void> {
  await page.addStyleTag({ content: CAPTURE_STYLESHEET });
  // Fonts settle after the stylesheet so metrics are final before we measure
  // overflow or take the shot.
  await page.evaluate(() => document.fonts?.ready);
  await page.waitForTimeout(150);
}

/** Locators for the global mask set plus any surface-specific additions. */
export function maskLocators(page: Page, extra: string[] = []): Locator[] {
  return [...NONDETERMINISTIC_SELECTORS, ...extra].map((sel) => page.locator(sel));
}

// ─────────────────────────── landing ───────────────────────────

/**
 * Load the app and wait for the landing state appropriate to the role.
 *
 * `fresh` lands on IdentitySetup (the onboarding gate). Warm roles restore
 * IndexedDB from storage state and land straight on the main UI, skipping the
 * 30-60s Groth16 proof.
 */
export async function landing(page: Page, role: AppRole): Promise<void> {
  await page.goto('/');
  await page.waitForLoadState('domcontentloaded');

  if (role === 'fresh') {
    await expect(page.getByRole('button', { name: 'Create New Identity' })).toBeVisible({
      timeout: 30_000,
    });
    return;
  }

  // Warm roles: identity, keys and the cached membership proof come back from
  // IndexedDB, but `serverReady` is component state in AppShell that a reload
  // resets. On web there is no auto-resume (unlike Tauri host mode), so a
  // returning user is shown the server chooser again even though they are
  // already registered with this server.
  //
  // That re-prompt is a real UX defect — tracked as `web-startup-no-resume`
  // in the findings ledger and fixed in the 03-server-startup stage. Until
  // then the harness clicks through it, exactly as a user has to.
  const chat = page.getByRole('button', { name: 'Chat' });
  const useThisServer = page.getByRole('button', { name: 'Continue' });

  const deadline = Date.now() + 90_000;
  while (Date.now() < deadline) {
    if (await chat.isVisible().catch(() => false)) break;
    if (await useThisServer.first().isVisible().catch(() => false)) {
      await useThisServer.first().click().catch(() => {});
    }
    await page.waitForTimeout(250);
  }

  await expect(chat).toBeVisible({ timeout: 10_000 });
  await expect(page.locator('.channel-list')).toBeVisible({ timeout: 20_000 });
}

// ─────────────────────────── main views ───────────────────────────

/** Switch the header tab. */
export async function openTab(page: Page, tab: 'Chat' | 'Federation' | 'Events'): Promise<void> {
  await page.getByRole('button', { name: tab }).click();
  if (tab === 'Chat') {
    await expect(page.locator('.app-layout')).toBeVisible();
  } else {
    await expect(page.locator('.view-content')).toBeVisible();
  }
}

export type AdminSection =
  | 'Server Settings'
  | 'Server Policy'
  | 'Member Management'
  | 'Channel Management';

/**
 * Open an admin section via the header gear dropdown.
 *
 * Only reachable as `founder` — the gear renders when `canModerate`, which
 * `ensure_founder` grants to the earliest registrant.
 */
export async function openAdminSection(page: Page, section: AdminSection): Promise<void> {
  const gear = page.locator('.admin-menu-btn');
  await expect(gear).toBeVisible({ timeout: 15_000 });
  await gear.click();
  await page.getByRole('button', { name: section }).click();
  await expect(page.locator('.view-content')).toBeVisible();
}

/** Open the admin dropdown without navigating, to capture the popover itself. */
export async function openAdminDropdown(page: Page): Promise<void> {
  await page.locator('.admin-menu-btn').click();
  await expect(page.locator('.admin-dropdown')).toBeVisible();
}

// ─────────────────────────── channels ───────────────────────────

/** Join (if needed) and select a channel by visible name. */
export async function selectChannel(page: Page, name: string): Promise<void> {
  const row = page.locator('.channel-item', { hasText: name });
  await expect(row).toBeVisible({ timeout: 15_000 });

  const join = row.locator('.join-btn');
  if (await join.isVisible().catch(() => false)) {
    await join.click();
    await expect(join).toBeHidden({ timeout: 15_000 });
  }

  await row.locator('.channel-select').click();
  await expect(row).toHaveClass(/active/, { timeout: 10_000 });
}

// ─────────────────────────── dialogs ───────────────────────────

/**
 * Click a StatusBar action and wait for its dialog.
 *
 * The gear button has no text, so it is addressed by class; the rest are
 * addressed by their visible label.
 */
export async function openStatusBarDialog(
  page: Page,
  action: 'audio' | 'Link' | 'Recovery' | 'Identity',
): Promise<void> {
  if (action === 'audio') {
    await page.locator('.status-actions button').first().click();
  } else {
    await page.locator('.status-actions').getByRole('button', { name: action }).click();
  }
  await expect(page.locator('.dialog')).toBeVisible({ timeout: 10_000 });
}

/** Wait for a dialog and return its root locator, for clipped captures. */
export async function dialog(page: Page): Promise<Locator> {
  const d = page.locator('.dialog').first();
  await expect(d).toBeVisible({ timeout: 10_000 });
  return d;
}
