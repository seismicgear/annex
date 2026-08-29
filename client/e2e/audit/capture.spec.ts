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

import { appendFileSync, mkdirSync } from 'node:fs';
import path from 'node:path';
import { expect, test } from '@playwright/test';
import { attachCollector, runAudits } from './audits';
import { landing, maskLocators, stabilize } from './nav';
import { storageStatePath, type WarmRole } from './roles';
import { SURFACES } from './surfaces';
import { VIEWPORTS, type Finding, type Viewport } from './types';

const AUDIT_ROOT = path.join(process.cwd(), 'e2e', 'audit');
const SHOT_DIR = path.join(AUDIT_ROOT, 'baselines');
// `reportOnly` surfaces are captured but never diffed, so their images are
// evidence rather than a reference. They used to be written into `baselines/`
// alongside the real ones, which meant every run rewrote tracked files that
// nothing compares against — `git status` came back dirty after a green run,
// and the noise is indistinguishable from a baseline someone meant to update.
// They live here instead, and this directory is gitignored.
const CAPTURE_DIR = path.join(AUDIT_ROOT, 'captures');
/** Failure evidence — never a baseline, so it stays out of the tracked set. */
const DIAG_DIR = path.join(AUDIT_ROOT, 'diagnostics');
const LEDGER_DIR = path.join(process.cwd(), '..', 'docs', 'ui-audit');
const LEDGER = path.join(LEDGER_DIR, 'findings.jsonl');

/**
 * Append findings to disk as they are produced, one JSON object per line.
 *
 * Accumulating in a module-level array and writing once in `afterAll` looked
 * simpler and was wrong: Playwright restarts the worker process after certain
 * test failures, which resets module state. A run with failures — exactly the
 * run whose findings matter most — reported an EMPTY ledger, because the
 * worker that finally ran `afterAll` had collected nothing.
 *
 * JSON Lines rather than JSON because appending must not require reading and
 * rewriting the whole file from several worker processes.
 * `scripts/ui-audit-report.mjs` consolidates it into `findings.json`.
 */
function record(...items: Finding[]): void {
  if (items.length === 0) return;
  mkdirSync(LEDGER_DIR, { recursive: true });
  appendFileSync(LEDGER, items.map((f) => JSON.stringify(f)).join('\n') + '\n');
}

function shotPath(surfaceId: string, viewport: Viewport, reportOnly = false): string {
  return path.join(reportOnly ? CAPTURE_DIR : SHOT_DIR, viewport.id, `${surfaceId}.png`);
}

/**
 * Capture warm-role surfaces first, `fresh` ones last.
 *
 * A `fresh` surface creates a brand-new identity, and every registration
 * moves the Merkle root. The warm roles hold a cached membership proof bound
 * to the root as it was before the run started; once it is superseded past
 * the server's 300s grace window the server rejects it, `loadPermissions`
 * comes back 403, and the founder silently loses the admin gear — so every
 * admin surface captured after an onboarding surface became unreachable.
 *
 * Ordering keeps the warm roles' proofs valid for their whole sweep. It does
 * not fix the underlying behaviour: a user whose proof goes stale while the
 * app is open also loses their capabilities until they reload, because the
 * client does not re-verify on a 403. That is tracked as a product gap.
 */
const ORDERED_SURFACES = [...SURFACES].sort((a, b) => {
  const rank = (r: string) => (r === 'fresh' ? 1 : 0);
  return rank(a.role) - rank(b.role);
});

for (const surface of ORDERED_SURFACES) {
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
        const file = shotPath(surface.id, viewport, surface.reportOnly);

        const shotOptions = {
          mask: maskLocators(page, surface.mask, surface.clip),
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
        record(...captured.map((f) => ({ ...f, screenshot: path.relative(AUDIT_ROOT, file) })));
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

        record({
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
        //
        // Waivers apply here too. `runAudits` honours `surface.waive`, and
        // this path did not, so a surface that injects a 500 to reach its own
        // error state reported that 500 as a finding the moment anything else
        // about it failed — noise in the ledger exactly when someone is
        // reading it to work out what actually went wrong.
        const waived = surface.waive ?? {};
        record(
          ...(waived.console ? [] : [...new Set(collector.pageErrors)]).map((detail) => ({
            surfaceId: surface.id,
            stage: surface.stage,
            viewport: viewport.id,
            audit: 'console' as const,
            severity: 'p1' as const,
            rule: 'uncaught-page-error',
            detail,
          })),
          ...(waived.network ? [] : [...new Set(collector.networkFailures)]).map((detail) => ({
            surfaceId: surface.id,
            stage: surface.stage,
            viewport: viewport.id,
            audit: 'network' as const,
            severity: 'p2' as const,
            rule: 'request-failed',
            detail,
          })),
        );
        // Recording is not enough: a surface we could not reach, or one that
        // no longer matches its baseline, is a failure and the run must say
        // so. Swallowing it here meant the lane reported green while ten
        // surfaces were unreachable — the ledger knew, and nothing read it.
        //
        // Audit FINDINGS (accessibility, contrast, overflow) stay
        // non-fatal by design, so one flawed screen does not truncate the
        // sweep. These are a different class.
        throw err;
      } finally {
        await context.close();
      }
    });
  }
}

