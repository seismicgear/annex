/**
 * Tauri IPC wrappers for desktop-specific functionality.
 *
 * These functions are only callable when the app is running inside a Tauri
 * webview. The `isTauri()` guard should be checked before invoking any command.
 */

import { invoke } from '@tauri-apps/api/core';

export interface StartupPrefsHost {
  startup_mode: { mode: 'host' };
}

export interface StartupPrefsClient {
  startup_mode: { mode: 'client'; server_url: string };
}

export type StartupPrefs = StartupPrefsHost | StartupPrefsClient;

/** Check if running inside a Tauri webview. */
export function isTauri(): boolean {
  return '__TAURI_INTERNALS__' in window;
}

/** Read saved startup mode preference. Returns null if none saved. */
export async function getStartupMode(): Promise<StartupPrefs | null> {
  return invoke<StartupPrefs | null>('get_startup_mode');
}

/** Save startup mode preference to disk. */
export async function saveStartupMode(prefs: StartupPrefs): Promise<void> {
  await invoke('save_startup_mode', { prefs });
}

/** Clear saved startup mode preference (reset). */
export async function clearStartupMode(): Promise<void> {
  await invoke('clear_startup_mode');
}

/** Start the embedded Axum server. Returns the server URL. */
export async function startEmbeddedServer(): Promise<string> {
  return invoke<string>('start_embedded_server');
}
