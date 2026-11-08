import { expect, type Locator, type Page } from '@playwright/test';

/** The armed action must remain readable and operable without hover. */
export async function expectDeleteConfirmation(page: Page, message: Locator): Promise<void> {
  const button = message.locator('.delete-btn.confirming');
  await page.mouse.move(0, 0);
  await expect(button).toBeFocused();
  await expect(button).toHaveText('Confirm?');
  await expect(message.locator('.message-actions')).toHaveCSS('opacity', '1');
  await expect.poll(async () => button.evaluate((element) => {
    const label = element.querySelector('.delete-confirm-text')!.getBoundingClientRect();
    const action = element.getBoundingClientRect();
    const bubble = element.closest('.message')!.getBoundingClientRect();
    const edit = element.parentElement!.querySelector('.edit-btn')!.getBoundingClientRect();
    const inside = (inner: DOMRect, outer: DOMRect) =>
      inner.left >= outer.left - 0.5 && inner.right <= outer.right + 0.5 &&
      inner.top >= outer.top - 0.5 && inner.bottom <= outer.bottom + 0.5;
    return {
      labelFitsButton: inside(label, action),
      buttonFitsMessage: inside(action, bubble),
      actionsDoNotOverlap: edit.right <= action.left || action.right <= edit.left ||
        edit.bottom <= action.top || action.bottom <= edit.top,
    };
  })).toEqual({ labelFitsButton: true, buttonFitsMessage: true, actionsDoNotOverlap: true });
}
