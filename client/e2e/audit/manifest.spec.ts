/**
 * Guards on the surface manifest itself.
 *
 * The manifest is the audit's coverage contract, and a coverage contract that
 * can drift silently is worth very little. These checks fail the audit lane
 * when the manifest stops describing the app — most importantly when someone
 * adds a dialog and forgets to declare it, which would otherwise mean a whole
 * screen quietly stops being screenshotted or audited.
 *
 * These are static checks over source files: no browser, no server.
 */

import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { expect, test } from '@playwright/test';
import { SURFACES } from './surfaces';
import { AUDIT_IDS, JOURNEY_STAGES, VIEWPORTS } from './types';

const SRC = path.join(process.cwd(), 'src');

function readAllSources(dir: string): { file: string; text: string }[] {
  const out: { file: string; text: string }[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) {
      out.push(...readAllSources(full));
    } else if (/\.tsx$/.test(entry.name) && !/\.test\.tsx$/.test(entry.name)) {
      out.push({ file: path.relative(SRC, full), text: readFileSync(full, 'utf8') });
    }
  }
  return out;
}

test.describe('surface manifest', () => {
  test('surface ids are unique', () => {
    const seen = new Map<string, number>();
    for (const s of SURFACES) seen.set(s.id, (seen.get(s.id) ?? 0) + 1);
    const duplicates = [...seen.entries()].filter(([, n]) => n > 1).map(([id]) => id);
    expect(duplicates, 'duplicate ids would overwrite each other’s screenshots').toEqual([]);
  });

  test('every surface declares a known stage, role and viewport set', () => {
    for (const s of SURFACES) {
      expect(JOURNEY_STAGES, `${s.id} has an unknown stage`).toContain(s.stage);
      expect(
        ['fresh', 'member', 'founder', 'second-member'],
        `${s.id} has an unknown role`,
      ).toContain(s.role);
      for (const v of s.viewports ?? []) {
        expect(
          VIEWPORTS.map((x) => x.id),
          `${s.id} references an unknown viewport`,
        ).toContain(v);
      }
    }
  });

  test('every surface explains itself', () => {
    // `intent` is what a reviewer reads in the contact sheet to know why a
    // screenshot exists. A blank one makes the capture unreviewable.
    for (const s of SURFACES) {
      expect(s.intent.trim().length, `${s.id} has no intent`).toBeGreaterThan(20);
      expect(s.title.trim().length, `${s.id} has no title`).toBeGreaterThan(3);
    }
  });

  test('every waiver names a real audit and gives a reason', () => {
    // The type already forces a string, but an empty or one-word waiver is
    // indistinguishable from silencing a bug. Require an actual sentence.
    for (const s of SURFACES) {
      for (const [audit, reason] of Object.entries(s.waive ?? {})) {
        expect(AUDIT_IDS, `${s.id} waives unknown audit "${audit}"`).toContain(audit);
        expect(
          reason.trim().length,
          `${s.id} waives ${audit} without explaining why`,
        ).toBeGreaterThan(15);
      }
    }
  });

  test('every stage that has surfaces is covered in journey order', () => {
    const covered = new Set(SURFACES.map((s) => s.stage));
    // 01-install is the desktop lane — unreachable from a browser against the
    // served SPA, so it is expected to be absent here.
    const expected = JOURNEY_STAGES.filter((s) => s !== '01-install');
    const missing = expected.filter((s) => !covered.has(s));
    expect(
      missing,
      'these journey stages have no captured surface at all — the audit would have a hole ' +
        'exactly where a user spends time',
    ).toEqual([]);
  });

  /**
   * The important one: a dialog that exists in the app but not in the manifest
   * is a screen nobody looks at.
   *
   * Detection is deliberately blunt — any component rendering a
   * `.dialog-overlay` is a modal — because a blunt check that fires is worth
   * more than a precise one nobody maintains. When this fails, either add the
   * surface or add the component to `KNOWN_UNREACHABLE` with a reason.
   */
  test('every modal component appears in the manifest', () => {
    /** Components that render a modal but are genuinely not reachable. */
    const KNOWN_UNREACHABLE: Record<string, string> = {
      'components/Modal.tsx':
        'the dialog primitive itself — it defines the overlay, it is not a screen. ' +
        'Every dialog that uses it is listed separately.',
    };

    // Three markers, not one.
    //
    // `dialog-overlay` alone found only the components that use the `Modal`
    // primitive, so a hand-rolled modal under its own class name was
    // invisible to the check whose entire job is finding modals nobody looks
    // at. Two were: the agent detail overlay and the image lightbox, each
    // covering the whole application, neither with a `role="dialog"`, focus
    // management, a focus trap or Escape. Both now use `Modal` — and the
    // detector no longer depends on their having done so.
    const MODAL_MARKERS = ['dialog-overlay', 'role="dialog"', "role='dialog'"];

    /**
     * Scan code, not prose.
     *
     * The markers are ordinary words, so a comment explaining why something
     * is *not* a dialog trips a plain `includes`. `Modal.tsx` only sits in
     * `KNOWN_UNREACHABLE` because its doc comment quotes the markup it
     * replaced. Stripping comments first makes the check answer the question
     * it is actually asking. `//` is left alone when preceded by `:` so that
     * URLs in strings survive — imprecise for a general parser, exact enough
     * for a boolean marker scan.
     */
    const stripComments = (text: string) =>
      text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1');

    const modalComponents = readAllSources(SRC)
      .map(({ file, text }) => ({ file, text, code: stripComments(text) }))
      .filter(({ code }) => MODAL_MARKERS.some((m) => code.includes(m)))
      .filter(({ file }) => !(file in KNOWN_UNREACHABLE));

    const haystack = SURFACES.map((s) => `${s.id} ${s.title}`.toLowerCase()).join(' | ');

    /** "CreateChannelDialog" -> ["create", "channel"] */
    const significantWords = (name: string) =>
      name
        .replace(/([a-z])([A-Z])/g, '$1 $2')
        .toLowerCase()
        .split(/\s+/)
        .filter((w) => !['dialog', 'panel', 'detail', 'overlay'].includes(w) && w.length > 2);

    const uncovered = modalComponents
      .filter(({ file, text }) => {
        // A file can host several modals — ServerHub declares AddServerDialog
        // inline, FederationPanel declares PeerDetail. Matching only the file
        // name would demand a surface called "server hub" for a dialog the
        // user knows as "add server". So each declared component in the file
        // is a candidate, and the file counts as covered when ANY of them is
        // reached.
        const candidates = [
          path.basename(file, '.tsx'),
          ...[...text.matchAll(/function\s+([A-Z][A-Za-z0-9]*)/g)].map((m) => m[1]),
        ];
        return !candidates.some((name) => {
          const words = significantWords(name);
          return words.length > 0 && words.every((w) => haystack.includes(w));
        });
      })
      .map(({ file }) => file);

    expect(
      uncovered,
      'these components render a modal that no surface in the manifest reaches — ' +
        'add a surface, or record why it is unreachable in KNOWN_UNREACHABLE',
    ).toEqual([]);
  });

  /**
   * The other half of coverage: not screens, but the STATES a screen reaches.
   *
   * Every modal is now in the manifest, and that says nothing about whether a
   * failed search, a failed scrollback or a refused send is ever looked at.
   * Those are the states this codebase gets wrong most often — a failure
   * rendered as an ordinary result is the single most common defect class
   * here — and they were invisible to every existing check.
   *
   * Detection is blunt on purpose, like the modal check: any class name
   * containing a state word is a state worth a picture. When this fails,
   * either add a surface that reaches it, or record it in `KNOWN_UNCOVERED`
   * with the reason.
   *
   * The stale-entry half matters as much as the missing-entry half. Without
   * it the allowlist only ever grows, and a list that only grows is a list
   * nobody removes anything from.
   */
  test('every state a screen can render is reached by some surface', () => {
    /**
     * States no surface reaches yet, each with why. Shrinking this list is
     * the work; adding to it needs a reason that is not "it was hard".
     */
    const KNOWN_UNCOVERED: Record<string, string> = {
      'admin-loading': 'transient — the policy fetch resolves before a capture could settle on it',
      'scrollback-error':
        'the scroll handler only fires when `hasMoreMessages` is true, which needs a first ' +
        'page of PAGE_SIZE (50) messages; the seed creates about ten, so reaching this ' +
        'state means stubbing a synthetic page and photographing invented content. The ' +
        'logic is covered by a store test instead.',
      'loading-text': 'transient — federation summary fetch, same reason',
      'startup-loading': 'transient — resolves as fast as the effect that sets it',
      'device-link-success': 'needs two contexts completing a real link handshake',
      'server-switch-error':
        'harder to reach than it looks, and the reason is worth keeping. A second server ' +
        'CAN be seeded — `seedSecondServer` in surfaces.ts does it, and ' +
        '`server-hub-registration-failed` uses it — but seeding one and clicking it does not ' +
        'produce this state: `switchServer` has almost nothing left in it that throws. ' +
        '`loadChannels` and `loadPermissions` both catch internally and set their own error ' +
        'state, and `selectIdentity` on an unknown id leaves the previous identity in place, ' +
        'so the switch reports success. Reaching it needs a stored identity with no ' +
        'pseudonymId, which is a state the app does not otherwise produce. The banner is ' +
        'pinned by four tests in ServerHub.switcherror.test.tsx instead.',
      'clear-state-error':
        'needs an IndexedDB delete to fail, which the harness has no way to force',
      'voice-error': 'covered in substance by voice-join-failure; the in-call variant is not',
      'voice-status-error': 'the StatusBar mirror of the same in-call voice error',
      'pending-status': 'the optimistic in-flight moment, gone before a capture settles',
    };

    /**
     * Class-name fragments that mark an element as a distinct visual STATE
     * rather than ordinary structure.
     */
    const STATE_WORDS =
      /error|empty|pending|failed|failure|loading|unavailable|success|offline|denied|expired|stale/;

    /**
     * Suffixes that name a CONTROL inside a state, not the state itself. The
     * state's own surface already photographs its dismiss button.
     */
    const IS_CONTROL = /-dismiss$|-btn$/;

    const stripComments = (text: string) =>
      text.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:])\/\/.*$/gm, '$1');

    const states = new Map<string, string[]>();
    for (const { file, text } of readAllSources(SRC)) {
      for (const m of stripComments(text).matchAll(/className=[{"'`]([^"'`}]*)/g)) {
        for (const raw of m[1].split(/\s+/)) {
          const cls = raw.replace(/[`${}]/g, '').trim();
          if (!cls || !STATE_WORDS.test(cls) || IS_CONTROL.test(cls)) continue;
          states.set(cls, [...(states.get(cls) ?? []), file]);
        }
      }
    }

    const manifest = readFileSync(path.join(process.cwd(), 'e2e/audit/surfaces.ts'), 'utf8');
    const uncovered = [...states.keys()]
      .filter((cls) => !manifest.includes(cls))
      .filter((cls) => !(cls in KNOWN_UNCOVERED))
      .sort();

    expect(
      uncovered,
      'no surface reaches these states, and they are not recorded as known gaps — ' +
        'add a surface that reaches one, or add it to KNOWN_UNCOVERED with a reason',
    ).toEqual([]);

    // And the reverse: an allowlist entry that IS now reached is a stale
    // excuse, and leaving it there hides the next real gap behind it.
    const staleExcuses = Object.keys(KNOWN_UNCOVERED)
      .filter((cls) => states.has(cls) && manifest.includes(cls))
      .sort();

    expect(
      staleExcuses,
      'these are listed as unreachable but a surface now reaches them — remove them ' +
        'from KNOWN_UNCOVERED',
    ).toEqual([]);
  });
});
