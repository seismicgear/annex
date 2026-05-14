/**
 * Tauri IPC wrappers for desktop-specific functionality.
 *
 * These functions are only callable when the app is running inside a Tauri
 * webview. The `isTauri()` guard should be checked before invoking any command.
 */

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

// ── Desktop origin constants ──

/**
 * All Tauri origins that a remote Annex server must allow in its CORS
 * `allowed_origins` list for the desktop app to connect.
 *
 * Keep in sync with `crates/annex-desktop/src/main.rs` and
 * `crates/annex-desktop/tauri.conf.json`.
 */
export const TAURI_DESKTOP_ORIGINS = [
  'tauri://localhost',
  'https://tauri.localhost',
  'http://tauri.localhost',
] as const;

/**
 * Human-readable CORS guidance for desktop users connecting to a remote server.
 * Used by StartupModeSelector and other error paths.
 */
export function desktopCorsGuidance(): string {
  const origins = TAURI_DESKTOP_ORIGINS.join(', ');
  return (
    'Could not reach server — this may be a CORS/origin configuration issue. ' +
    'The remote server needs to allow Tauri desktop origins in its CORS allowed_origins. ' +
    `Ask the server admin to add all desktop origins: ${origins}`
  );
}

// ── Public endpoint response ──

export interface PublicEndpointInfo {
  public_url: string;
  public_webrtc_url: string | null;
}

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

/**
 * Reset server data directory (database, uploads, config).
 *
 * Called on fresh install detection to ensure stale data from a previous
 * installation doesn't persist. Must be called before the embedded server
 * starts.
 */
export async function resetServerData(): Promise<void> {
  await invoke('reset_server_data');
}

/** Start the embedded Axum server. Returns the server URL. */
export async function startEmbeddedServer(): Promise<string> {
  return invoke<string>('start_embedded_server');
}

/** Register with the Annex router to acquire a public endpoint. Returns the public URL. */
export async function acquirePublicEndpoint(): Promise<string> {
  return invoke<string>('acquire_public_endpoint');
}

/** Get the current public endpoint info, if a router session is active. */
export async function getPublicEndpoint(): Promise<PublicEndpointInfo | null> {
  return invoke<PublicEndpointInfo | null>('get_public_endpoint');
}

/** Open a native save dialog and export identity JSON to disk. */
export async function exportIdentityJson(json: string): Promise<string | null> {
  return invoke<string | null>('export_identity_json', { json });
}

// ── WebRTC configuration ──

export interface WebRtcSettings {
  configured: boolean;
  url: string;
  api_key: string;
  has_api_secret: boolean;
  token_ttl_seconds: number;
}

/** Read the current WebRTC configuration status. */
export async function getWebRtcConfig(): Promise<WebRtcSettings> {
  return invoke<WebRtcSettings>('get_webrtc_config');
}

/** Start a local WebRTC server. Returns the WebRTC WebSocket URL. */
export async function startLocalWebRtc(): Promise<{ url: string }> {
  return invoke<{ url: string }>('start_local_webrtc');
}

/**
 * Clear the in-process WebRTC config override so the embedded server falls
 * back to whatever's in `config.toml` (typically empty for desktop installs)
 * when WebRTC actually failed to start. Must be called BEFORE
 * `startEmbeddedServer()`.
 */
export async function clearWebRtcEnv(): Promise<void> {
  return invoke<void>('clear_webrtc_env');
}

// ── Platform media status ──

export interface PlatformMediaStatus {
  /** Screen sharing readiness: boolean or tri-state string. */
  screen_share_available: boolean | 'available' | 'unknown' | 'blocked';
  /** Tri-state: `true`/`"available"` = verified, `"unknown"` = may need grant, `false`/`"blocked"` = unavailable. */
  camera_mic_available: boolean | 'available' | 'unknown' | 'blocked';
  warnings: string[];
  display_server: string;
}

/** Query platform media capabilities (PipeWire, screen sharing, etc.). */
export async function getPlatformMediaStatus(): Promise<PlatformMediaStatus> {
  return invoke<PlatformMediaStatus>('get_platform_media_status');
}

// ── Media keepalive ──

/**
 * Tell the Rust backend to keep the webview's IsVisible=true even when the
 * window is minimized. This prevents WebView2/Chromium from killing active
 * MediaStreamTracks (mic, camera, screen share) during a voice call.
 *
 * Call with `true` when joining a call, `false` when leaving.
 */
export async function setMediaKeepalive(active: boolean): Promise<void> {
  await invoke('set_media_keepalive', { active });
}

// ── Cold-start invite retrieval ──

/**
 * Retrieve and clear a buffered cold-start invite from managed Rust state.
 *
 * During app launch the deep-link URL may arrive before the React event
 * listener mounts. This command fetches the invite that was parsed during
 * Tauri's `setup()` phase. Returns `null` if no invite was buffered.
 * The buffer is cleared on read so the invite is processed exactly once.
 */
export async function getPendingInvite(): Promise<{ server: string; code: string } | null> {
  return invoke<{ server: string; code: string } | null>('get_pending_invite');
}

// ── First-run installation marker ──

/** Check whether first-run initialization has completed previously. */
export async function checkFirstRunCompleted(): Promise<boolean> {
  return invoke<boolean>('check_first_run_completed');
}

/** Write the first-run marker so subsequent launches skip fresh-install cleanup. */
export async function markFirstRunCompleted(): Promise<void> {
  await invoke('mark_first_run_completed');
}

// ── WebRTC reachability check ──

/** Check if a WebRTC server is reachable at the given URL. */
export async function checkWebRtcReachable(url: string): Promise<{ reachable: boolean; error?: string }> {
  return invoke<{ reachable: boolean; error?: string }>('check_webrtc_reachable', { url });
}

// ── Deep-link invite listener ──

export async function listenForInvite(
  callback: (invite: { server: string; code: string }) => void,
): Promise<() => void> {
  const unlisten = await listen<{ server: string; code: string }>('annex-invite', (event) => {
    callback(event.payload);
  });
  return unlisten;
}
