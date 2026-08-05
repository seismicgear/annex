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
      'components/ProfileSwitcher.tsx':
        'dead code — zero imports, superseded by IdentitySettings',
      'components/UsernameSettings.tsx':
        'dead code — zero imports, superseded by IdentitySettings',
    };

    const modalComponents = readAllSources(SRC)
      .filter(({ text }) => text.includes('dialog-overlay'))
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
});
