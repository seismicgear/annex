/**
 * Web/Docker startup preference helpers.
 *
 * Extracted from StartupModeSelector so that component file only exports
 * React components (required by react-refresh/only-export-components).
 */

export const STARTUP_MODE_STORAGE_KEY = 'annex:startup-mode';

export interface WebStartupPrefs {
  mode: 'local' | 'remote';
  server_url?: string;
}

export function loadWebStartupMode(): WebStartupPrefs | null {
  try {
    const raw = localStorage.getItem(STARTUP_MODE_STORAGE_KEY);
    return raw ? (JSON.parse(raw) as WebStartupPrefs) : null;
  } catch {
    return null;
  }
}

export function saveWebStartupMode(prefs: WebStartupPrefs): void {
  try {
    localStorage.setItem(STARTUP_MODE_STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // Storage full or blocked — non-fatal.
  }
}

/** Clear the saved startup preference (called on logout). */
export function clearWebStartupMode(): void {
  try {
    localStorage.removeItem(STARTUP_MODE_STORAGE_KEY);
  } catch {
    // Storage unavailable — non-fatal.
  }
}
