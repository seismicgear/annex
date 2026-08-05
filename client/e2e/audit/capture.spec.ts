/**
 * The capture runner.
 *
 * Walks the surface manifest and, for every (surface × viewport) pair:
 *   1. installs the surface's stubs, sizes the viewport, loads the role's warm
 *      state and drives the UI to the surface,
 *   2. stabilises the frame (animations off, nondeterministic regions masked),
 *   3. writes a screenshot,
 *   4. runs the automated audit battery and appends to the findings ledger.
 *
 * Findings are recorded rather than asserted. An exhaustive audit is only
 * useful if it reaches the end, so a surface with an accessibility violation
 * still gets captured and the run still covers everything after it. The
 * ledger is the deliverable; `report.mjs` renders it.
 */

import { mkdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { expect, test } from '@playwright/test';
import { attachCollector, runAudits } from './audits';
import { landing, maskLocators, stabilize } from './nav';
import { storageStatePath, type WarmRole } from './roles';
import { SURFACES } from './surfaces';
import { VIEWPORTS, type Finding, type Viewport } from './types';

const AUDIT_ROOT = path.join(process.cwd(), 'e2e', 'audit');
const SHOT_DIR = path.join(AUDIT_ROOT, 'baselines');
/** Failure evidence — never a baseline, so it stays out of the tracked set. */
const DIAG_DIR = path.join(AUDIT_ROOT, 'diagnostics');
const LEDGER_DIR = path.join(process.cwd(), '..', 'docs', 'ui-audit');

const findings: Finding[] = [];

function shotPath(surfaceId: string, viewport: Viewport): string {
  return path.join(SHOT_DIR, viewport.id, `${surfaceId}.png`);
}

for (const surface of SURFACES) {
  const viewports = surface.viewports
    ? VIEWPORTS.filter((v) => surface.viewports!.includes(v.id))
    : VIEWPORTS;

  for (const viewport of viewports) {
    test(`${surface.stage} · ${surface.id} @ ${viewport.id}`, async ({ browser }) => {
      const context = await browser.newContext({
        viewport: { width: viewport.width, height: viewport.height },
        storageState:
          surface.role === 'fresh'
            ? undefined
            : storageStatePath(surface.role as WarmRole),
        // Grant media up front so the voice surfaces render their real state
        // rather than a permission prompt we did not mean to capture.
        permissions: ['microphone', 'camera'],
      });

      const page = await context.newPage();
      const collector = attachCollector(page);

      try {
        await surface.setup?.(page);
        await landing(page, surface.role);
        await surface.navigate(page);
        await stabilize(page);

        const target = surface.clip ? page.locator(surface.clip) : page;
        const file = shotPath(surface.id, viewport);

        const shotOptions = {
          mask: maskLocators(page, surface.mask),
          maskColor: '#3a3a3a',
          animations: 'disabled' as const,
          // Font hinting and sub-pixel AA differ enough between machines that
          // a zero-tolerance compare would fail on every host but the one that
          // recorded the baseline. 0.5% is tight enough to catch a colour,
          // spacing or layout change and loose enough to survive that.
          maxDiffPixelRatio: 0.005,
        };

        if (surface.reportOnly) {
          // Genuinely nondeterministic beyond what masking can fix (live
          // video, animated media). Still captured and audited — just not
          // diffed, so it cannot produce phantom failures.
          mkdirSync(path.dirname(file), { recursive: true });
          await target.screenshot({ path: file, ...shotOptions });
        } else {
          // `toHaveScreenshot` writes the baseline on first run and compares
          // on every run after, so unintended visual drift fails CI. New
          // baselines are therefore an explicit act
          // (`ui-audit.sh --update-baselines`), not a silent side effect.
          await expect(target).toHaveScreenshot([viewport.id, `${surface.id}.png`], shotOptions);
        }

        const captured = await runAudits(page, surface, viewport.id, collector);
        for (const f of captured) {
          findings.push({ ...f, screenshot: path.relative(AUDIT_ROOT, file) });
        }
      } catch (err) {
        // Two very different failures land here and the ledger must not blur
        // them: a visual diff means the surface rendered but changed, while
        // anything else means we never got there at all — a navigation recipe
        // that has drifted from the UI, or a UI that is broken.
        //
        // Either way, recording it keeps the rest of the run going and puts
        // the failure in the ledger next to everything else instead of losing
        // it in a stack trace.
        const message = err instanceof Error ? err.message : String(err);
        const isVisualDiff = /toHaveScreenshot|Screenshot comparison failed/i.test(message);

        // A best-effort screenshot of wherever we ended up is usually the
        // fastest way to tell which. It goes to the diagnostics directory,
        // not `baselines/` — it is evidence about a failure, not an approved
        // picture of a working screen.
        const file = path.join(DIAG_DIR, viewport.id, `${surface.id}.png`);
        mkdirSync(path.dirname(file), { recursive: true });
        await page.screenshot({ path: file }).catch(() => {});

        findings.push({
          surfaceId: surface.id,
          stage: surface.stage,
          viewport: viewport.id,
          audit: 'console',
          severity: isVisualDiff ? 'p2' : 'p1',
          rule: isVisualDiff ? 'visual-regression' : 'surface-unreachable',
          detail: isVisualDiff
            ? `this surface no longer matches its committed baseline. If the change is ` +
              `intended, re-record with \`bash scripts/ui-audit.sh --update-baselines\`. ` +
              `${message.split('\n')[0]}`
            : `could not reach this surface: ${message.split('\n')[0]}`,
          screenshot: path.relative(AUDIT_ROOT, file),
        });

        // Console/network signals collected on the way are often the reason.
        for (const e of new Set(collector.pageErrors)) {
          findings.push({
            surfaceId: surface.id,
            stage: surface.stage,
            viewport: viewport.id,
            audit: 'console',
            severity: 'p1',
            rule: 'uncaught-page-error',
            detail: e,
          });
        }
        for (const e of new Set(collector.networkFailures)) {
          findings.push({
            surfaceId: surface.id,
            stage: surface.stage,
            viewport: viewport.id,
            audit: 'network',
            severity: 'p2',
            rule: 'request-failed',
            detail: e,
          });
        }
      } finally {
        await context.close();
      }
    });
  }
}

test.afterAll(() => {
  mkdirSync(LEDGER_DIR, { recursive: true });
  // Sorted so the ledger diffs cleanly between runs: stage, then surface,
  // then viewport, then severity.
  const severityRank = { p1: 0, p2: 1, p3: 2 } as const;
  findings.sort(
    (a, b) =>
      a.stage.localeCompare(b.stage) ||
      a.surfaceId.localeCompare(b.surfaceId) ||
      a.viewport.localeCompare(b.viewport) ||
      severityRank[a.severity] - severityRank[b.severity] ||
      a.rule.localeCompare(b.rule),
  );
  writeFileSync(
    path.join(LEDGER_DIR, 'findings.json'),
    `${JSON.stringify({ generatedBy: 'client/e2e/audit/capture.spec.ts', findings }, null, 2)}\n`,
  );
});
