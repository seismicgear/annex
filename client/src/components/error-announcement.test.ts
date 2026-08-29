/**
 * Every error message the app renders must announce itself.
 *
 * A source-scanning contract rather than 21 render tests, in the same spirit
 * as `e2e/audit/manifest.spec.ts`: the point is that no NEW error surface is
 * added without a live region, which no per-component test can enforce.
 *
 * The defect it pins: an error inserted into the page with no `role="alert"`
 * and no enclosing live region is invisible to a screen reader. The user
 * presses Save, hears nothing, and has no way to tell the action failed from
 * the action succeeding quietly. Twenty-one such elements existed across
 * eleven components while twenty-nine others were announced correctly.
 */
import { describe, it, expect } from 'vitest';
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

/** `client/src`, resolved from this file's own location. */
const SRC = resolve(dirname(fileURLToPath(import.meta.url)), '..');

/** Class names that mark an element as user-facing error TEXT. */
const ERROR_CLASS = /\berror-message\b|\bform-error\b|\berror-text\b|\bchannel-encryption-error\b/;
/** Dismiss controls carry error-ish names but are buttons, not the message. */
const DISMISS_CLASS = /-dismiss\b/;
const LIVE = /role="alert"|role="status"|aria-live=/;

function tsxFiles(dir: string): string[] {
  return readdirSync(dir).flatMap((name) => {
    const full = join(dir, name);
    if (statSync(full).isDirectory()) return tsxFiles(full);
    return name.endsWith('.tsx') && !name.includes('.test.') ? [full] : [];
  });
}

function indent(line: string): number {
  return line.length - line.trimStart().length;
}

/**
 * Elements whose live region is not textually above them. `MessageView`
 * routes every branch through a `shell()` helper carrying `role="log"` and
 * `aria-live="polite"`, so its error paragraphs are already announced.
 */
const ANNOUNCED_BY_HELPER = ['MessageView.tsx'];

describe('error messages are announced', () => {
  it('has no error text outside a live region', () => {
    const offenders: string[] = [];

    for (const file of tsxFiles(SRC)) {
      if (ANNOUNCED_BY_HELPER.some((f) => file.endsWith(f))) continue;
      const lines = readFileSync(file, 'utf8').split('\n');

      lines.forEach((line, i) => {
        if (!line.includes('className=') || line.includes('role=')) return;
        const m = /className="([^"]*)"/.exec(line);
        if (!m) return;
        const cls = m[1];
        if (DISMISS_CLASS.test(cls) || !ERROR_CLASS.test(cls)) return;

        // Walk the ancestor chain by strictly decreasing indentation. Only
        // lines shallower than everything seen so far can enclose this one;
        // siblings sit at the same depth and are skipped. Stopping at the
        // first shallower line instead would stop at a preceding sibling.
        let depth = indent(line);
        let announced = false;
        for (let j = i - 1; j >= 0 && j > i - 40 && depth > 0; j--) {
          if (!lines[j].trim()) continue;
          const d = indent(lines[j]);
          if (d >= depth) continue;
          depth = d;
          if (LIVE.test(lines[j])) {
            announced = true;
            break;
          }
        }
        if (announced) return;
        offenders.push(`${file.slice(SRC.length + 1)}:${i + 1}  .${cls}`);
      });
    }

    expect(
      offenders,
      'These error messages are inserted into the page with nothing to announce them. ' +
        'Add role="alert" to the element, or wrap a group of them in one alert — ' +
        'not one per line, which reads as several interruptions describing one event.',
    ).toEqual([]);
  });
});
