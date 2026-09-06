/**
 * The accent palette, defined once.
 *
 * This list existed in three places byte-for-byte identical: `personas.ts`,
 * `servers.ts`, and `ProfileSwitcher.tsx` — where it had already drifted, which
 * is what a duplicated constant does eventually. The colour is not decoration:
 * a persona's accent drives its buttons, badges, avatar and highlights across
 * the whole app, and a server's drives its hub icon, so two lists that disagree
 * means the same identity renders in two different colours depending on which
 * module happened to assign it.
 */

export const ACCENT_COLORS = [
  '#e63946',
  '#646cff',
  '#4ade80',
  '#f87171',
  '#fbbf24',
  '#7eb8da',
  '#b87eda',
  '#ff6b9d',
  '#c42836',
  '#10b981',
  '#6366f1',
  '#ec4899',
] as const;

/**
 * Pick an accent at random.
 *
 * Deliberately random rather than derived from the identity: a colour derived
 * from a pseudonym would leak that pseudonym's identity across personas that
 * are supposed to be unlinkable.
 */
export function randomAccentColor(): string {
  return ACCENT_COLORS[Math.floor(Math.random() * ACCENT_COLORS.length)];
}
