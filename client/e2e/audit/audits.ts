/**
 * The automated checks run against every captured surface.
 *
 * A screenshot proves what a surface looks like; these prove whether it works.
 * Each check returns `Finding[]` rather than asserting, so one broken surface
 * produces a complete ledger entry instead of aborting the run at the first
 * failure — an exhaustive audit is only useful if it gets to the end.
 */

import AxeBuilder from '@axe-core/playwright';
import type { Page, Request, Response } from '@playwright/test';
import type { AuditId, Finding, Severity, Surface, ViewportId } from './types';

/**
 * Collects passive signals (console errors, page errors, failed requests) for
 * the whole time a surface is being reached, not just at the moment of
 * capture. Attach before navigation.
 */
export interface Collector {
  consoleErrors: string[];
  pageErrors: string[];
  networkFailures: string[];
  reset(): void;
}

/** Requests whose failure is expected and carries no signal. */
function isIgnorableRequest(url: string): boolean {
  return (
    // Favicon is not shipped; a 404 here says nothing about the UI.
    url.endsWith('/favicon.ico') ||
    // Playwright aborts in-flight requests on navigation/teardown.
    url.startsWith('data:') ||
    url.startsWith('blob:')
  );
}

export function attachCollector(page: Page): Collector {
  const collector: Collector = {
    consoleErrors: [],
    pageErrors: [],
    networkFailures: [],
    reset() {
      this.consoleErrors.length = 0;
      this.pageErrors.length = 0;
      this.networkFailures.length = 0;
    },
  };

  page.on('console', (msg) => {
    if (msg.type() === 'error') collector.consoleErrors.push(msg.text());
  });
  page.on('pageerror', (err) => {
    collector.pageErrors.push(err.message);
  });
  page.on('requestfailed', (req: Request) => {
    const url = req.url();
    if (isIgnorableRequest(url)) return;
    collector.networkFailures.push(`${req.method()} ${url} — ${req.failure()?.errorText ?? 'failed'}`);
  });
  page.on('response', (res: Response) => {
    const url = res.url();
    if (isIgnorableRequest(url)) return;
    if (res.status() >= 400) {
      collector.networkFailures.push(`${res.request().method()} ${url} — HTTP ${res.status()}`);
    }
  });

  return collector;
}

// ─────────────────────────── individual audits ───────────────────────────

const AXE_TAGS = ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa', 'best-practice'];

function axeSeverity(impact: string | null | undefined): Severity {
  switch (impact) {
    case 'critical':
      return 'p1';
    case 'serious':
      return 'p2';
    default:
      return 'p3';
  }
}

async function auditA11y(page: Page): Promise<Omit<Finding, 'surfaceId' | 'stage' | 'viewport'>[]> {
  const results = await new AxeBuilder({ page }).withTags(AXE_TAGS).analyze();
  return results.violations.map((v) => ({
    audit: 'a11y' as const,
    severity: axeSeverity(v.impact),
    rule: v.id,
    detail: v.help,
    target: v.nodes[0]?.target?.join(' ') ?? undefined,
  }));
}

function auditConsole(c: Collector): Omit<Finding, 'surfaceId' | 'stage' | 'viewport'>[] {
  const findings: Omit<Finding, 'surfaceId' | 'stage' | 'viewport'>[] = [];
  for (const e of new Set(c.pageErrors)) {
    findings.push({
      audit: 'console',
      severity: 'p1',
      rule: 'uncaught-page-error',
      detail: e,
    });
  }
  for (const e of new Set(c.consoleErrors)) {
    findings.push({
      audit: 'console',
      severity: 'p2',
      rule: 'console-error',
      detail: e,
    });
  }
  return findings;
}

function auditNetwork(c: Collector): Omit<Finding, 'surfaceId' | 'stage' | 'viewport'>[] {
  return [...new Set(c.networkFailures)].map((detail) => ({
    audit: 'network' as const,
    severity: 'p2' as const,
    rule: 'request-failed',
    detail,
  }));
}

/**
 * Layout integrity: does anything push the page sideways, and is any text
 * clipped by its own container?
 *
 * The horizontal-overflow check is the one that will light up at narrow
 * widths — App.css has zero responsive breakpoints, so this is expected to
 * report at `mobile` until that is addressed. That is the point: the harness
 * documents the gap rather than asserting it away.
 */
async function auditOverflow(page: Page): Promise<Omit<Finding, 'surfaceId' | 'stage' | 'viewport'>[]> {
  return page.evaluate(() => {
    const findings: { audit: 'overflow'; severity: 'p2' | 'p3'; rule: string; detail: string; target?: string }[] = [];
    const doc = document.documentElement;

    if (doc.scrollWidth > doc.clientWidth + 1) {
      findings.push({
        audit: 'overflow',
        severity: 'p2',
        rule: 'horizontal-overflow',
        detail: `document scrollWidth ${doc.scrollWidth} exceeds clientWidth ${doc.clientWidth}`,
      });
    }

    // Text clipped by an ancestor with hidden overflow and no ellipsis. Only
    // report elements that actually hold text, to avoid flagging scroll
    // containers doing their job.
    const seen = new Set<string>();
    for (const el of Array.from(document.querySelectorAll<HTMLElement>('body *'))) {
      const style = getComputedStyle(el);
      if (style.overflow !== 'hidden' && style.overflowX !== 'hidden') continue;
      if (style.textOverflow === 'ellipsis') continue;
      // Screen-reader-only text is clipped to 1px on purpose — that is the
      // whole technique. Flagging it would be reporting the fix as the bug.
      if (el.classList.contains('visually-hidden')) continue;
      if (!el.textContent?.trim()) continue;
      if (el.scrollWidth <= el.clientWidth + 1) continue;
      if (el.children.length > 0) continue; // leaf text nodes only

      const target = el.className ? `.${String(el.className).split(/\s+/).join('.')}` : el.tagName;
      if (seen.has(target)) continue;
      seen.add(target);

      findings.push({
        audit: 'overflow',
        severity: 'p3',
        rule: 'clipped-text',
        detail: `text clipped without ellipsis (scrollWidth ${el.scrollWidth} > clientWidth ${el.clientWidth})`,
        target,
      });
    }
    return findings;
  });
}

/**
 * Dialog keyboard contract: Escape closes, and focus stays inside while open.
 *
 * Only runs for surfaces that render a `.dialog`. Annex has no focus traps
 * today and handles Escape in exactly two components, so this is expected to
 * report widely on the first run.
 */
async function auditKeyboard(page: Page): Promise<Omit<Finding, 'surfaceId' | 'stage' | 'viewport'>[]> {
  const dialog = page.locator('.dialog').first();
  if (!(await dialog.isVisible().catch(() => false))) return [];

  const findings: Omit<Finding, 'surfaceId' | 'stage' | 'viewport'>[] = [];

  // A dialog should be announced as one.
  const hasRole = await dialog.evaluate(
    (el) => el.getAttribute('role') === 'dialog' || el.closest('[role="dialog"]') !== null,
  );
  if (!hasRole) {
    findings.push({
      audit: 'keyboard',
      severity: 'p2',
      rule: 'dialog-missing-role',
      detail: 'modal container has no role="dialog", so assistive tech does not announce it as modal',
    });
  }

  // Focus should be inside the dialog when it opens, otherwise keyboard users
  // start from wherever they were and may never reach it.
  const focusInside = await dialog.evaluate(
    (el) => document.activeElement !== null && el.contains(document.activeElement),
  );
  if (!focusInside) {
    findings.push({
      audit: 'keyboard',
      severity: 'p2',
      rule: 'dialog-focus-not-moved',
      detail: 'focus was not moved into the dialog when it opened',
    });
  }

  // Tabbing repeatedly must not escape the dialog.
  let escaped = false;
  for (let i = 0; i < 25; i++) {
    await page.keyboard.press('Tab');
    const inside = await dialog
      .evaluate((el) => document.activeElement !== null && el.contains(document.activeElement))
      .catch(() => true);
    if (!inside) {
      escaped = true;
      break;
    }
  }
  if (escaped) {
    findings.push({
      audit: 'keyboard',
      severity: 'p2',
      rule: 'dialog-focus-not-trapped',
      detail: 'Tab moved focus outside the open dialog — focus is not trapped',
    });
  }

  // Escape should dismiss.
  await page.keyboard.press('Escape');
  const stillOpen = await dialog.isVisible().catch(() => false);
  if (stillOpen) {
    findings.push({
      audit: 'keyboard',
      severity: 'p2',
      rule: 'dialog-escape-does-not-close',
      detail: 'pressing Escape did not close the dialog',
    });
  }

  return findings;
}

// ─────────────────────────── driver ───────────────────────────

/**
 * Run the full battery against the currently-rendered surface.
 *
 * Waived audits are skipped and recorded as such in the run log, so a waiver
 * is visible rather than an invisible hole in coverage.
 */
export async function runAudits(
  page: Page,
  surface: Surface,
  viewport: ViewportId,
  collector: Collector,
): Promise<Finding[]> {
  const waived = surface.waive ?? {};
  const raw: Omit<Finding, 'surfaceId' | 'stage' | 'viewport'>[] = [];

  const run = async (id: AuditId, fn: () => Promise<Omit<Finding, 'surfaceId' | 'stage' | 'viewport'>[]>) => {
    if (waived[id]) return;
    try {
      raw.push(...(await fn()));
    } catch (err) {
      raw.push({
        audit: id,
        severity: 'p3',
        rule: 'audit-crashed',
        detail: `${id} audit threw: ${err instanceof Error ? err.message : String(err)}`,
      });
    }
  };

  await run('a11y', () => auditA11y(page));
  await run('overflow', () => auditOverflow(page));
  // Keyboard runs last among the interactive checks because it presses Escape
  // and can close the surface under test.
  await run('keyboard', () => auditKeyboard(page));

  if (!waived.console) raw.push(...auditConsole(collector));
  if (!waived.network) raw.push(...auditNetwork(collector));

  return raw.map((f) => ({
    ...f,
    surfaceId: surface.id,
    stage: surface.stage,
    viewport,
  }));
}
