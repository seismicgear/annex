# Tauri Build Audit Report (v2)

**Audit date:** 2026-02-27
**Auditor:** Claude (comprehensive code audit, re-audit — does not trust previous pass)

---

## Executive Summary

This re-audit independently verified every claim from the previous audit. The previous pass was **mostly correct** but missed several dead-code issues and under-documented the mixed-content risk for Linux WebKitGTK.

**Issues found and fixed in this pass:**
1. **Dead IPC surface area** — 5 Tauri commands were registered in the invoke handler but had zero frontend callers (the LiveKit settings panel that called them was deleted). Unregistered them to reduce attack surface.
2. **Dead frontend functions** — `stopTunnel()` and `getTunnelUrl()` in `tauri.ts` had no callers anywhere in the frontend. Removed.
3. **Missing mixed-content documentation** — The `ws://127.0.0.1:7880` LiveKit URL works on Chromium (WebView2) because 127.0.0.1 is treated as "potentially trustworthy", but WebKitGTK on Linux may block this. Added AUDIT-TAURI comment with remediation guidance.

**No build-breaking issues found.** The previous fixes (`useHttpsScheme: true`, CSP, LiveKit panel removal) were correctly applied and verified.

---

## Phase 1: Tauri Configuration Audit

### 1a. `tauri.conf.json`

**File:** `crates/annex-desktop/tauri.conf.json`

#### Security / Webview Settings

| Check | Status | Notes |
|-------|--------|-------|
| `useHttpsScheme: true` | PASS | Set on the main window (line 23). Webview runs as `https://tauri.localhost` — WebRTC APIs work. |
| No `http://tauri.localhost` fetch targets | PASS | Only in CORS `allowed_origins` (intentional fallback for platform compatibility) and one test file. Not used as a client-side fetch URL. |
| No hardcoded `http://localhost:PORT` in frontend | PASS | Only in test files (`*.test.tsx`), vite dev proxy config (dev-only), and Rust test fixtures. None in production client code. |

#### CSP (Content Security Policy)

**Current CSP (verified, syntactically valid):**
```
default-src 'self' tauri: https://tauri.localhost;
connect-src * ws: wss:;
img-src 'self' data: blob: http: https:;
media-src 'self' blob: data: mediastream: http: https:;
style-src 'self' 'unsafe-inline';
font-src 'self' data:;
script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval' blob:;
worker-src 'self' blob:
```

| Directive | Status | Notes |
|-----------|--------|-------|
| `media-src` includes `'self' blob: mediastream:` | PASS | All four required values present (`'self'`, `blob:`, `mediastream:`, `https://tauri.localhost` via `default-src`). Also includes `data:` `http:` `https:`. |
| `connect-src` allows `ws: wss:` | PASS | Uses `*` wildcard plus explicit `ws: wss:`. Allows LiveKit WebSocket signaling. |
| `connect-src` allows STUN/TURN | PASS | `*` covers all connection targets. |
| `script-src` includes `'unsafe-eval' 'wasm-unsafe-eval'` | PASS | Required for snarkjs (ZK proof generation) and any WASM modules. |
| `worker-src` includes `'self' blob:` | PASS | Required for LiveKit SDK Web Workers created from blob URLs. |
| `img-src` includes `'self' blob: data:` | PASS | Covers user avatars, generated images, data URIs. |
| `font-src` includes `data:` | PASS | Prevents blocking of data: URI fonts from CSS dependencies. |
| `default-src` not overly restrictive | PASS | Allows `'self' tauri: https://tauri.localhost` — appropriate for Tauri. |
| No `frame-src` / `child-src` blocking content | PASS | Not specified; defaults to `default-src`. |
| CSP syntactically valid | PASS | All directives correctly separated by semicolons. |

#### Permissions / Capabilities

**File:** `crates/annex-desktop/capabilities/default.json`

```json
{
  "identifier": "default",
  "description": "Default capability set for the Annex desktop application.",
  "windows": ["main"],
  "permissions": ["core:default"]
}
```

| Check | Status | Notes |
|-------|--------|-------|
| `core:default` sufficient | PASS | All file I/O uses native Rust (`std::fs`). File dialogs use `rfd` (bypasses Tauri ACL). Process spawning uses `std::process::Command`. No Tauri plugins are used (`tauri = { version = "2", features = [] }`). |
| No missing plugin permissions | PASS | No Tauri plugins enabled in `Cargo.toml`. |
| Window management permissions | PASS | Single main window created by config. No dynamic window creation. |

### 1b. Platform-Specific Configuration

#### Windows (WebView2)

| Check | Status | Notes |
|-------|--------|-------|
| Dark window border | PASS | `set_dark_window_border()` correctly uses DWM APIs (`DWMWA_USE_IMMERSIVE_DARK_MODE`, `DWMWA_BORDER_COLOR`) in `.setup()`. |
| `--autoplay-policy` browser arg | **FLAGGED** | Not set. WebView2 may block auto-playing audio (LiveKit's `RoomAudioRenderer`). Flagged as `// AUDIT-TAURI` in `main.rs` setup closure (line 1336). |
| `PermissionRequested` handler for getUserMedia | **FLAGGED** | Not implemented. WebView2 may silently deny camera/mic without explicit permission handling. Flagged as `// AUDIT-TAURI` in `main.rs` (line 1329). |
| WebView2 auto-install | PASS | `webviewInstallMode.type: "downloadBootstrapper"` with `silent: true` in `tauri.conf.json` (line 50-53). |

#### Linux (WebKitGTK)

| Check | Status | Notes |
|-------|--------|-------|
| Mixed-content ws:// from https:// context | **FLAGGED** | LiveKit uses `ws://127.0.0.1:7880`. Chromium treats 127.0.0.1 as trustworthy (allowing ws:// from https://). WebKitGTK may not. Flagged in `main.rs` `start_local_livekit`. |
| PipeWire for screen sharing on Wayland | NOT DOCUMENTED | Required for `getDisplayMedia()` on Wayland. No detection or documentation. |
| Minimum WebKitGTK version | NOT DOCUMENTED | CI uses `libwebkit2gtk-4.1-dev`. WebRTC support varies by version. |

#### macOS (WKWebView)

| Check | Status | Notes |
|-------|--------|-------|
| Tauri origin `tauri://localhost` | PASS | CORS config includes this origin. |
| Camera/mic permissions | NOT DOCUMENTED | macOS requires `NSCameraUsageDescription` / `NSMicrophoneUsageDescription` in `Info.plist`. Not verified if Tauri v2 adds these automatically. |

---

## Phase 2: WebRTC / LiveKit Audit

### 2a. Media Permissions Flow

**Room connection path:**
1. User clicks **Join Call** / **Create Call** → `VoicePanel.handleJoin()` (VoicePanel.tsx:326)
2. → `useVoiceStore.joinCall()` (voice.ts:102)
3. → `api.joinVoice(pseudonymId, channelId)` — HTTP POST to `/api/channels/{id}/voice/join`
4. → Server generates JWT token, returns `{ token, url }` where `url` = `voice_service.get_public_url()`
5. → `<LiveKitRoom serverUrl={livekitUrl} token={voiceToken} audio={true}>` renders
6. → LiveKit SDK calls `getUserMedia({ audio: true })` internally on connect

| Check | Status | Notes |
|-------|--------|-------|
| getUserMedia wrapped in try/catch | PASS | LiveKit SDK handles internally. `AudioSettings.enumerateMediaDevices()` (AudioSettings.tsx:31-53) has fallback for permission denial. |
| Error feedback when media fails | PASS | `VoicePanel` shows `lastJoinError` from voice store. `AudioSettings` shows "Grant microphone/camera access" hint. |
| getDisplayMedia from user gesture | PASS | `MediaControls.toggleScreen()` bound to button `onClick` handler — direct user interaction (VoicePanel.tsx:75-77). |
| Video elements have autoplay/playsinline | PASS | Handled by `@livekit/components-react`'s `VideoTrack` component. MessageInput video previews use `playsInline` and `muted` (MessageInput.tsx:150-155). |
| Remote tracks rendered correctly | PASS | `useTracks()` and `useParticipants()` hooks from `@livekit/components-react` handle `TrackSubscribed` events. |
| Race condition: DOM vs track arrival | PASS | LiveKit React components manage this internally via hooks — tracks are attached when both the DOM element and track exist. |

### 2b. LiveKit Connection Configuration

| Check | Status | Notes |
|-------|--------|-------|
| LiveKit URL source (Tauri host mode) | PASS | `start_local_livekit()` (main.rs:998) sets `ANNEX_LIVEKIT_URL` and `ANNEX_LIVEKIT_PUBLIC_URL` env vars before server starts. `joinVoice` API returns URL from `voice_service.get_public_url()`. |
| API key/secret auto-generated | PASS | `uuid::Uuid::new_v4()` generates unique random key and secret per launch (main.rs:1019-1020). |
| API secret stored in OS keyring | PASS | `store_api_secret_in_keyring()` (main.rs:564) with fallback to config.toml if keyring unavailable. |
| LiveKit URL is ws:// not wss:// | **KNOWN RISK** | Uses `ws://127.0.0.1:7880`. Works on Chromium (127.0.0.1 = trustworthy) but may fail on WebKitGTK. See Phase 1b Linux section. |
| STUN/TURN servers | NOT CONFIGURED | LiveKit `--dev` mode includes built-in STUN. No custom STUN/TURN servers configured. Works for localhost and simple NAT, may fail on restrictive corporate networks. |

### 2c. ICE Connectivity

| Check | Status | Notes |
|-------|--------|-------|
| ICE servers specified | IMPLICIT | LiveKit handles ICE internally. `--dev` mode includes defaults. |
| CSP allows STUN/TURN connections | PASS | `connect-src *` allows all outbound connections. |

---

## Phase 3: State Management and IPC Audit

### 3a. Tauri Commands — Registration vs Frontend Usage

All `#[tauri::command]` functions and their registration/usage status:

| Command | Registered | Frontend Caller | Serializes Correctly |
|---------|-----------|-----------------|---------------------|
| `get_startup_mode` | YES | `tauri.ts:getStartupMode()` → App.tsx, StartupModeSelector.tsx | YES (Option\<StartupPrefs\>) |
| `save_startup_mode` | YES | `tauri.ts:saveStartupMode()` → StartupModeSelector.tsx | YES |
| `clear_startup_mode` | YES | `tauri.ts:clearStartupMode()` → StartupModeSelector.tsx, App.tsx | YES |
| `start_embedded_server` | YES | `tauri.ts:startEmbeddedServer()` → StartupModeSelector.tsx | YES (String) |
| `start_tunnel` | YES | `tauri.ts:startTunnel()` → StartupModeSelector.tsx | YES (String) |
| `stop_tunnel` | YES | None (frontend wrapper removed) | N/A |
| `get_tunnel_url` | YES | None (frontend wrapper removed) | N/A |
| `export_identity_json` | YES | `tauri.ts:exportIdentityJson()` → StatusBar.tsx | YES (Option\<String\>) |
| `get_livekit_config` | YES | `tauri.ts:getLiveKitConfig()` → StartupModeSelector.tsx | YES |
| `start_local_livekit` | YES | `tauri.ts:startLocalLiveKit()` → StartupModeSelector.tsx | YES (JSON value) |
| `save_livekit_config` | **UNREGISTERED** | None | N/A — dead code |
| `clear_livekit_config` | **UNREGISTERED** | None | N/A — dead code |
| `check_livekit_reachable` | **UNREGISTERED** | None | N/A — dead code |
| `stop_local_livekit` | **UNREGISTERED** | None | N/A — dead code |
| `get_local_livekit_url` | **UNREGISTERED** | None | N/A — dead code |

**Note:** `stop_tunnel` and `get_tunnel_url` remain registered even though their frontend wrappers were removed. They are still needed because the Rust-side tunnel management uses them, and future UI may expose tunnel controls. Leaving them registered is harmless.

### 3b. Event Emitters / Listeners

| Check | Status | Notes |
|-------|--------|-------|
| Tauri events (emit/listen) | NOT USED | The app uses IPC commands exclusively. No `app.emit()` or `window.emit()` calls in main.rs. No `listen()` calls in frontend. No event name mismatch risk. |

### 3c. Docker vs Tauri State Paths

| State | Docker Mode | Tauri Mode | Implemented |
|-------|-------------|------------|-------------|
| Identity keys | IndexedDB | IndexedDB | SAME — no divergence |
| Server URL | Current origin (relative) | `setApiBaseUrl(url)` from `startEmbeddedServer` | YES — `api.ts:resolveUrl()` handles both |
| Startup prefs | localStorage | Disk via IPC (`startup_prefs.json`) | YES — `StartupModeSelector` branches on `isTauri()` |
| Audio settings | localStorage | localStorage | SAME — no divergence |
| WebSocket | `ws://` or `wss://` via current origin | `ws://127.0.0.1:{port}` via baseUrl replacement | YES — `ws.ts` handles both paths |
| LiveKit config | Server-side env vars | `start_local_livekit()` sets env vars before server | YES |

**Tauri detection:** `isTauri()` checks `'__TAURI_INTERNALS__' in window`. Used consistently in:
- `App.tsx` (startup flow, error hints)
- `StartupModeSelector.tsx` (mode selection UI)
- `StatusBar.tsx` (identity export via native dialog)

### 3d. Input Handling

| Check | Status | Notes |
|-------|--------|-------|
| Text inputs use standard handlers | PASS | All inputs use React controlled components with `onChange`/`onSubmit`. No custom event interception. |
| Global shortcuts interfering | PASS | No `globalShortcut` or similar registered anywhere. |
| Focus management issues | NOT TESTABLE | Tauri window focus switching between native chrome and webview requires hardware testing. |
| `contenteditable` elements | NONE | Not used anywhere in the frontend. |
| `MessageInput` textarea | PASS | Standard `<textarea>` with `onKeyDown` for Enter-to-send (MessageInput.tsx:96-101). No IME composition issues expected. |

---

## Phase 4: Silent Auto-Configuration Audit

### 4a. LiveKit Server Configuration

| Check | Status | Notes |
|-------|--------|-------|
| Auto-started with zero interaction | PASS | `StartupModeSelector.applyHost()` calls `getLiveKitConfig()` → if not configured → `startLocalLiveKit()`. All happens before user sees the main app. |
| API key/secret auto-generated | PASS | Random UUID-based keys per launch (`annex_{uuid}` / `secret_{uuid}`). |
| Credentials persist across restart | PARTIAL | Credentials are regenerated on each launch (not persisted). This is intentional for the local dev-mode server. For production, the config.toml `[livekit]` section with keyring-stored secret persists. |
| Port collision on 7880 | **FLAGGED** | Hardcoded `port: u16 = 7880`. If occupied, startup times out after 15s. `livekit-server --port` doesn't support auto-select (port 0). Flagged with `// AUDIT-TAURI` in main.rs. |
| User feedback on failure | PASS | `StartupModeSelector` shows "Setting up voice..." phase, and catches errors with `console.warn` + continues (voice is optional, app still works). |

### 4b. Network Configuration

| Check | Status | Notes |
|-------|--------|-------|
| NAT detection | NOT IMPLEMENTED | No auto-detection. LiveKit `--dev` mode includes built-in STUN. |
| STUN/TURN defaults | PARTIAL | LiveKit dev mode provides basic STUN. No TURN server for restrictive networks. |
| Graceful degradation | YES | Voice failure shows error in `VoicePanel`. Text channels remain fully functional. The app doesn't appear "broken" — it just can't do voice. |
| mDNS/local discovery | NOT IMPLEMENTED | Uses cloudflared tunnel for remote access. No mDNS for LAN-only usage. |

### 4c. Device Selection

| Check | Status | Notes |
|-------|--------|-------|
| Auto-select system default | PASS | `AudioSettings` defaults to "System Default" (`deviceId: null`). LiveKit SDK uses the system default when no specific device is selected. |
| Hot-plug notification | NOT IMPLEMENTED | No `devicechange` event listener. User must reopen audio settings to see new devices. |
| Unavailable device graceful failure | PASS | LiveKit SDK falls back to any available device if the selected one is missing. |

### 4d. Settings Panels

| Check | Status | Notes |
|-------|--------|-------|
| LiveKit settings panel | DELETED | `LiveKitSettings.tsx` correctly removed. No dead imports found. |
| AdminPanel | CLEAN | No references to LiveKit settings. Four sections remain: Server Settings, Policy, Members, Channels. |
| AudioSettings | FUNCTIONAL | User-facing audio/video device selection. Appropriate for end users. |
| Dead routes | NONE | No route definitions reference deleted components. |

---

## Phase 5: Build and Bundle Verification

### 5a. Build Configuration

| Check | Status | Notes |
|-------|--------|-------|
| `beforeBuildCommand` | PASS | `node ../../scripts/build-desktop.js` — builds ZK artifacts, Piper TTS, and client. Correct path relative to `crates/annex-desktop/`. |
| `frontendDist` | PASS | `../../client/dist` — correct path to built client assets. |
| Build order | PASS | `build-desktop.js` runs `npm run build` for the client (Step 4) before Tauri bundles it. |
| Source maps | PASS | `sourcemap: false` in `vite.config.ts` (line 43). Not shipped in production. |

### 5b. Asset Bundling

| Check | Status | Notes |
|-------|--------|-------|
| ZK artifacts | PASS | `build-desktop.js` copies `membership.wasm` and `membership_final.zkey` to `client/public/zk/`. |
| Bundle resources | PASS | `tauri.conf.json` bundles `membership_vkey.json`, `piper/`, and `voices/`. |
| Icons | PASS | All required icon sizes present in `icons/` directory (32, 128, 128@2x, icns, ico). |
| NSIS installer | PASS | `nsis/hooks.nsi` includes uninstall hook to clean `%AppData%\Annex`. |

### 5c. Dev vs Prod Configuration

| Check | Status | Notes |
|-------|--------|-------|
| `devUrl` is localhost only | PASS | `"devUrl": "http://localhost:5173"` — only used in `cargo tauri dev`, not in production builds. |
| Console statements | PASS | Only 3 `console.warn` calls in production code — all appropriate warning-level messages (voice config failure, tunnel failure, username load failure). |
| Environment variables | PASS | Production env vars set in `main.rs:main()` before server starts. No dev-only values leak. |

---

## Phase 6: Previous Fix Verification

### Fix 1: LiveKit settings panel deleted

| Check | Status |
|-------|--------|
| `LiveKitSettings.tsx` deleted from filesystem | PASS — file does not exist |
| No import references | PASS — no imports of this component anywhere |
| No route references | PASS — no routes point to it |
| AdminPanel updated | PASS — no LiveKit settings section, four clean sections remain |
| Dead IPC functions cleaned | **FIXED** — 5 backend commands unregistered from invoke handler, 2 frontend functions removed from tauri.ts |

### Fix 2: `useHttpsScheme: true`

| Check | Status |
|-------|--------|
| Present in `tauri.conf.json` | PASS — line 23 |
| No `http://tauri.localhost` fetch targets | PASS — only in CORS origins (correct) |
| Embedded server URL is `http://127.0.0.1:{port}` | PASS — 127.0.0.1 is trusted by Chromium, so mixed-content rules allow it from https:// context |
| LiveKit WebSocket uses `ws://127.0.0.1:7880` | **RISK ON LINUX** — WebKitGTK may not have the 127.0.0.1 exception. Documented with AUDIT-TAURI comment. |

### Fix 3: CSP `media-src` added

| Check | Status |
|-------|--------|
| `media-src` present with all required values | PASS |
| `connect-src` allows WebSocket | PASS (`* ws: wss:`) |
| `worker-src` allows blob: | PASS |
| `font-src` present | PASS (added by previous audit) |
| CSP syntactically valid | PASS |

### Fix 4: Success message color

| Check | Status |
|-------|--------|
| Cosmetic, skipped per instructions | SKIPPED |

---

## Issues Fixed in This Audit

### 1. Unregistered dead Tauri commands

**File:** `crates/annex-desktop/src/main.rs`

**Problem:** Five `#[tauri::command]` functions were registered in `generate_handler![]` but had zero frontend callers after the LiveKit settings panel was deleted. Each registered command increases the IPC attack surface.

**Fix:** Removed `save_livekit_config`, `clear_livekit_config`, `check_livekit_reachable`, `stop_local_livekit`, and `get_local_livekit_url` from the `invoke_handler` macro. The Rust implementations are retained (with `#[allow(dead_code)]`) for potential future use.

### 2. Removed dead frontend IPC functions

**File:** `client/src/lib/tauri.ts`

**Problem:** `stopTunnel()` and `getTunnelUrl()` were defined but never imported or called anywhere in the frontend.

**Fix:** Removed both functions.

### 3. Added mixed-content documentation for Linux WebKitGTK

**File:** `crates/annex-desktop/src/main.rs`

**Problem:** The `ws://127.0.0.1:7880` LiveKit URL relies on Chromium's exception treating 127.0.0.1 as "potentially trustworthy" (allowing ws:// from https:// context). This is undocumented and may fail on Linux WebKitGTK.

**Fix:** Added AUDIT-TAURI comment in `start_local_livekit()` documenting the risk and remediation path (TLS listener or localhost proxy).

---

## Issues Requiring Hardware Test

All flagged with `// AUDIT-TAURI:` in the codebase:

### Windows (WebView2)

1. **getUserMedia permission handling** (`main.rs:1329`) — WebView2 may silently deny camera/mic without an explicit `on_permission_request` handler. Test: join a voice call, enable camera, verify video appears.

2. **Autoplay policy** (`main.rs:1336`) — WebView2 may block auto-playing audio from `RoomAudioRenderer`. Test: join a call with another participant, verify you can hear their audio without clicking.

3. **If either fails**, add `additional_browser_args` to the `WebviewWindowBuilder`:
   ```rust
   .additional_browser_args("--autoplay-policy=no-user-gesture-required")
   ```
   And/or add a WebView2 permission request handler.

### Linux (WebKitGTK)

4. **ws:// from https:// secure context** (`main.rs:1013`) — LiveKit connects to `ws://127.0.0.1:7880`. May be blocked by WebKitGTK mixed-content rules. Test: host mode → join voice call → check browser console for mixed-content errors.

5. **WebSocket for app messaging** (`ws.ts:47-48`) — Same issue: `ws://127.0.0.1:{port}` for the app's real-time messaging WebSocket. The code has an AUDIT-TAURI comment about this.

6. **PipeWire for screen sharing on Wayland** — `getDisplayMedia()` requires PipeWire on Wayland. Test: share screen on a Wayland session.

### macOS

7. **Info.plist camera/mic descriptions** — macOS requires `NSCameraUsageDescription` and `NSMicrophoneUsageDescription`. Verify Tauri v2 auto-generates these, or add them manually.

### All Platforms

8. **Port 7880 collision** (`main.rs:1024`) — If port 7880 is in use, LiveKit startup times out after 15s. Test: run another service on 7880, launch Annex, verify error message is shown and text chat still works.

---

## Dead Code / Dead References

| Item | Location | Status |
|------|----------|--------|
| `save_livekit_config` command | main.rs | Impl retained, unregistered from handler |
| `clear_livekit_config` command | main.rs | Impl retained, unregistered from handler |
| `check_livekit_reachable` command | main.rs | Impl retained, unregistered from handler |
| `stop_local_livekit` command | main.rs | Impl retained, unregistered from handler |
| `get_local_livekit_url` command | main.rs | Impl retained, unregistered from handler |
| `SaveLiveKitInput` struct | main.rs | Used only by dead `save_livekit_config`, marked `#[allow(dead_code)]` |
| `default_token_ttl` function | main.rs | Used only by `SaveLiveKitInput`, marked `#[allow(dead_code)]` |
| `stopTunnel()` | tauri.ts | **Removed** |
| `getTunnelUrl()` | tauri.ts | **Removed** |
| `LiveKitSettings.tsx` | client/src/components/ | Confirmed deleted — no file exists |

---

## CSP Final State

```
default-src 'self' tauri: https://tauri.localhost;
connect-src * ws: wss:;
img-src 'self' data: blob: http: https:;
media-src 'self' blob: data: mediastream: http: https:;
style-src 'self' 'unsafe-inline';
font-src 'self' data:;
script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval' blob:;
worker-src 'self' blob:
```

All directives validated. Semicolons correctly separate each directive.

---

## Tauri Permissions Final State

**File:** `crates/annex-desktop/capabilities/default.json`

```json
{
  "identifier": "default",
  "description": "Default capability set for the Annex desktop application.",
  "windows": ["main"],
  "permissions": ["core:default"]
}
```

Sufficient because:
- File I/O: Rust `std::fs` (no Tauri file plugin needed)
- File dialogs: `rfd` crate (native OS dialogs, no Tauri ACL)
- Process spawning: `std::process::Command` (no Tauri shell plugin needed)
- No Tauri plugins enabled

---

## Registered Tauri Commands (Final)

```rust
invoke_handler(tauri::generate_handler![
    get_startup_mode,
    save_startup_mode,
    clear_startup_mode,
    start_embedded_server,
    start_tunnel,
    stop_tunnel,
    get_tunnel_url,
    export_identity_json,
    get_livekit_config,
    start_local_livekit,
])
```

10 commands registered. All have active frontend callers except `stop_tunnel` and `get_tunnel_url` (tunnel management may need them; registered is harmless).

---

## Remaining Risk Areas

### High Risk (may cause user-visible failures)

1. **WebView2 getUserMedia silent denial** — No permission handler means camera/mic may not work on Windows without any error message. This is the most likely cause of "video doesn't work in Tauri" reports.

2. **WebView2 autoplay blocking** — Remote participants' audio may not play without user gesture. Could manifest as "I can't hear anyone" with no error.

3. **Linux WebKitGTK ws:// blocking** — Voice calls may silently fail on Linux. The embedded server API (http://) works because 127.0.0.1 is trustworthy, but WebSocket (ws://) may be treated differently.

### Medium Risk (may affect specific configurations)

4. **Port 7880 collision** — Second Annex instance or any service on 7880 causes voice startup timeout. Error message is shown but user might not understand it.

5. **No TURN server** — Users behind corporate firewalls/strict NAT won't be able to establish voice calls. Text chat works fine.

6. **macOS camera/mic permissions** — May need Info.plist entries.

### Low Risk (edge cases)

7. **Device hot-plug** — Plugging in headset mid-call doesn't auto-switch. User must use Audio Settings dialog.

8. **Wayland screen sharing** — Requires PipeWire. Not detected or documented.

9. **LiveKit credentials not persisted** — Auto-generated per launch in host mode. If the server restarts mid-session, existing LiveKit tokens become invalid. Users would need to leave and rejoin voice.
