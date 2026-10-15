import { randomUUID } from 'node:crypto';
import { expect, type Page } from '@playwright/test';
import { openAdminSection, selectChannel } from './nav';

const channels = new WeakMap<Page, string>();

/** Each capture owns a real channel; encryption must never mutate General. */
export async function createEncryptionChannel(page: Page): Promise<string> {
  const name = `audit-encryption-${randomUUID()}`;
  await page.locator('.create-channel-btn').click();
  const dialog = page.getByRole('dialog');
  await dialog.getByPlaceholder('general').fill(name);
  const response = page.waitForResponse(value =>
    new URL(value.url()).pathname === '/api/channels' && value.request().method() === 'POST');
  await dialog.getByRole('button', { name: 'Create', exact: true }).click();
  const created = await response;
  expect(created.ok(), 'The isolated encryption channel was created by the real server').toBe(true);
  channels.set(page, name);
  await expect(dialog).toBeHidden();
  expect(await created.json()).toEqual({ status: 'created' });
  expect(created.request().postDataJSON()).toMatchObject({ channel_id: name, name });
  await selectChannel(page, name);
  await expect(page.getByRole('button', { name: '🔒 Enable encryption', exact: true })).toBeEnabled();
  return name;
}

/** Enroll the real device, sign the manifest and retrieve the actual sealed key. */
export async function enableEncryption(page: Page): Promise<void> {
  await page.getByRole('button', { name: '🔒 Enable encryption', exact: true }).click();
  await expect(page.locator('.channel-encryption-bar.encrypted.key-ready')).toBeVisible({ timeout: 15_000 });
}

/**
 * Fault injection preserves the server's certified v2 state and signed history.
 * Missing wraps models withheld delivery; a 500 models an unavailable key service.
 * Reload clears the manager's memory and proves the UI handles a fresh key read.
 */
export async function interruptKeyDelivery(page: Page, channel: string, state: 'pending' | 'failed'): Promise<void> {
  let reads = 0;
  await page.route(url => url.pathname === `/api/channels/${channel}/key-wraps`, async route => {
    if (route.request().method() !== 'GET') return route.continue();
    reads++;
    if (state === 'failed') {
      await route.fulfill({ status: 500, contentType: 'application/json', body: '{"error":"key delivery unavailable"}' });
      return;
    }
    const response = await route.fetch();
    expect(response.ok()).toBe(true);
    const body = await response.json();
    expect(body.state.version).toBe(2);
    expect(body.state.has_key).toBe(true);
    // Do not fabricate key material, certificates, counters, or manifest signatures.
    await route.fulfill({ response, json: { ...body, wraps: [], next_after_epoch: null,
      legacy_device_wraps: [], next_legacy_cursor: null, legacy_wraps: [], next_legacy_wrap_after_epoch: null } });
  });
  await page.reload();
  await selectChannel(page, channel);
  await expect(page.locator(`.channel-encryption-bar.encrypted.key-${state}`)).toBeVisible({ timeout: 15_000 });
  expect(reads, 'The intended real key-delivery request reached the fault injection').toBeGreaterThan(0);
  await expect(page.locator('.channel-encryption-bar.key-ready')).toHaveCount(0);
  if (state === 'pending') {
    await expect(page.locator('.channel-encryption-bar [role="status"]')).toHaveText(
      'Encrypted. Waiting for this device’s channel keys from an authorized member.',
    );
    await expect(page.getByRole('button', { name: 'Check again', exact: true })).toBeEnabled();
  } else {
    await expect(page.locator('.channel-encryption-bar [role="alert"]')).toContainText(
      'The channel key could not be loaded: key delivery unavailable. Sending remains blocked.',
    );
    await expect(page.getByRole('button', { name: 'Retry', exact: true })).toBeEnabled();
  }
}

/** Cleanup through the real deletion UI, even when a capture assertion fails. */
export async function removeEncryptionChannel(page: Page): Promise<void> {
  const name = channels.get(page);
  if (!name) return;
  if (await page.getByRole('dialog').isVisible()) {
    await page.keyboard.press('Escape');
    await expect(page.getByRole('dialog')).toBeHidden();
  }
  await openAdminSection(page, 'Channel Management');
  const row = page.locator('.channel-manager-item', { hasText: name });
  await expect(row).toBeVisible();
  await row.getByRole('button', { name: 'Delete', exact: true }).click();
  await page.getByRole('dialog').getByRole('button', { name: 'Delete channel', exact: true }).click();
  await expect(row).toHaveCount(0);
  channels.delete(page);
}
