/**
 * Web/Docker startup preference helpers.
 *
 * Extracted from StartupModeSelector so that component file only exports
 * React components (required by react-refresh/only-export-components).
 */

const STORAGE_KEY = 'annex:startup-mode';

/** Clear the saved startup preference (called on logout). */
export function clearWebStartupMode(): void {
  try {
    localStorage.removeItem(STORAGE_KEY);
  } catch {
    // Storage unavailable — non-fatal.
  }
}
