import { expect, type Page } from '@playwright/test';

/** Measure actual header controls, including hit testing the legal trigger. */
export async function measureHeaderControls(page: Page) {
  const selectors = [
    '.app-header h1',
    '.header-tabs',
    '.product-legal-trigger',
    '.persona-context-indicator',
    '.admin-menu-btn',
  ];
  for (const selector of selectors) await expect(page.locator(selector)).toBeVisible();
  return page.evaluate((controls) => {
    const items = controls.map((selector) => {
      const box = document.querySelector(selector)!.getBoundingClientRect();
      return {
        selector, left: box.left, right: box.right, top: box.top,
        bottom: box.bottom, width: box.width, height: box.height,
      };
    });
    const overlaps = [];
    for (let a = 0; a < items.length; a++) {
      for (let b = a + 1; b < items.length; b++) {
        const width = Math.min(items[a].right, items[b].right) - Math.max(items[a].left, items[b].left);
        const height = Math.min(items[a].bottom, items[b].bottom) - Math.max(items[a].top, items[b].top);
        if (width > 0.5 && height > 0.5) overlaps.push({ a: items[a].selector, b: items[b].selector, width, height });
      }
    }
    const header = document.querySelector('.app-header')!.getBoundingClientRect();
    const legal = document.querySelector('.product-legal-trigger')!;
    const box = legal.getBoundingClientRect();
    const hit = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2);
    return {
      items, overlaps, headerHeight: header.height,
      legalCenterUnobstructed: legal === hit || legal.contains(hit),
      horizontalOverflow: document.documentElement.scrollWidth > innerWidth,
      controlsInHeader: items.every((item) => item.left >= header.left && item.right <= header.right &&
        item.top >= header.top && item.bottom <= header.bottom),
    };
  }, selectors);
}

/** A visible label is insufficient when the centered persona intercepts it. */
export async function expectHeaderControlsAccessible(page: Page) {
  const geometry = await measureHeaderControls(page);
  expect(geometry.overlaps, 'Header controls must not overlap').toEqual([]);
  expect(geometry.legalCenterUnobstructed, 'Legal control must receive pointer input').toBe(true);
  expect(geometry.horizontalOverflow, 'Header must not widen the document').toBe(false);
  expect(geometry.controlsInHeader, 'Header controls must fit inside the header').toBe(true);
  return geometry;
}
