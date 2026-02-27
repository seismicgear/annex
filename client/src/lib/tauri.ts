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

/** Start a cloudflared tunnel to expose the local server. Returns the public URL. */
export async function startTunnel(): Promise<string> {
  return invoke<string>('start_tunnel');
}

/** Stop the cloudflared tunnel if running. */
export async function stopTunnel(): Promise<void> {
  await invoke('stop_tunnel');
}

/** Get the current tunnel URL, if a tunnel is active. */
export async function getTunnelUrl(): Promise<string | null> {
  return invoke<string | null>('get_tunnel_url');
}

/** Open a native save dialog and export identity JSON to disk. */
export async function exportIdentityJson(json: string): Promise<string | null> {
  return invoke<string | null>('export_identity_json', { json });
}

// ── LiveKit configuration ──

export interface LiveKitSettings {
  configured: boolean;
  url: string;
  api_key: string;
  has_api_secret: boolean;
  token_ttl_seconds: number;
}

/** Read the current LiveKit configuration status. */
export async function getLiveKitConfig(): Promise<LiveKitSettings> {
  return invoke<LiveKitSettings>('get_livekit_config');
}

/** Start a local LiveKit server. Returns the LiveKit WebSocket URL. */
export async function startLocalLiveKit(): Promise<{ url: string }> {
  return invoke<{ url: string }>('start_local_livekit');
}
