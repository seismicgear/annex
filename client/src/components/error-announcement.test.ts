/**
 * Every error and every success the app renders must announce itself.
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

/**
 * Class names that mark an element as a user-facing state MESSAGE.
 *
 * Successes count for the same reason errors do, and were missed on the first
 * pass: a screen reader user presses Save, hears nothing, and cannot tell a
 * silent success from a silent failure. `role="status"` (polite) is the right
 * one there — `role="alert"` interrupts, which a confirmation should not.
 */
const ERROR_CLASS =
  /\berror-message\b|\bform-error\b|\berror-text\b|\bchannel-encryption-error\b|\bsuccess-message\b|\bdevice-link-success\b/;
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
        'Add role="alert" (errors) or role="status" (successes) to the element, or wrap a ' +
        'group of them in one — not one per line, which reads as several interruptions ' +
        'describing a single event.',
    ).toEqual([]);
  });

  /**
   * Having a live region is not the same as having something to announce.
   *
   * `ServerHub` satisfied the rule above with
   * `<div className="server-hub-error" role="alert" title={switchError}>`
   * wrapping `<span>!</span>`. A screen reader announced the character "!".
   * The sentence explaining what had failed sat in a `title`, which is
   * hover-only on a pointer and unreachable entirely on touch — so the one
   * place the message existed was the one place half the users could not
   * look. The first rule cannot see this, because the role is present.
   */
  it('puts no live-region message in a title attribute', () => {
    const offenders: string[] = [];

    for (const file of tsxFiles(SRC)) {
      readFileSync(file, 'utf8')
        .split('\n')
        .forEach((line, i) => {
          if (!/role="(alert|status)"/.test(line)) return;
          if (!/\btitle=[{"]/.test(line)) return;
          offenders.push(`${file.slice(SRC.length + 1)}:${i + 1}`);
        });
    }

    expect(
      offenders,
      'A live region announces its text content, not its title attribute, and a title ' +
        'is hover-only on a pointer and unreachable on touch. Put the message in the ' +
        'element (visually hidden if it must not be seen), not in a tooltip.',
    ).toEqual([]);
  });
});
