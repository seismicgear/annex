# Tauri Build Audit Report

Audit date: 2026-02-27
Auditor: Claude (automated code audit)

---

## Executive Summary

The Tauri desktop build had **one critical build-breaking bug** (wrong `beforeBuildCommand` path), **one production leak** (source maps shipped in bundle), and **one missing CSP directive** (`font-src`). These are now fixed.

The previous commit's core changes (`useHttpsScheme: true`, CSP `media-src`, LiveKit settings panel removal) were **correctly applied** and verified. However, several platform-specific behaviors (WebView2 permission handling, autoplay policy, WebKitGTK mixed-content rules) cannot be verified without hardware testing and are flagged with `// AUDIT-TAURI:` comments in the code.

---

## Phase 1: Tauri Configuration Audit

### 1a. `tauri.conf.json`

**File:** `crates/annex-desktop/tauri.conf.json`

#### Security / Webview Settings

| Check | Status | Notes |
|-------|--------|-------|
| `useHttpsScheme: true` | PASS | Set on the main window. WebRTC APIs will work in secure context. |
| No `http://tauri.localhost` in codebase | PASS | Only found in CORS allowed_origins (intentional fallback) and one test file. Not used as a fetch target. |
| No hardcoded `http://localhost:PORT` in frontend | PASS | Only in test files and vite dev proxy config (dev-only). |

#### CSP (Content Security Policy)

**Final CSP after fixes:**
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
| `media-src` includes `'self' blob: mediastream:` | PASS | All four required values present. Also includes `data: http: https:`. |
| `connect-src` allows `ws: wss:` | PASS | Uses `*` wildcard plus explicit `ws: wss:`. |
| `script-src` includes `'unsafe-eval' 'wasm-unsafe-eval'` | PASS | Required for snarkjs (ZK proofs) and any WASM modules. |
| `worker-src` includes `blob:` | PASS | Required for LiveKit SDK Web Workers. |
| `img-src` includes `blob: data:` | PASS | Covers user avatars and generated content. |
| `font-src` includes `data:` | **FIXED** | Was missing (fell back to `default-src` which lacks `data:`). Added `font-src 'self' data:` to prevent blocking of data: URI fonts from CSS dependencies. |
| `default-src` not overly restrictive | PASS | Allows `'self' tauri: https://tauri.localhost`. |
| No `frame-src` / `child-src` blocking content | PASS | Not specified; defaults to `default-src`. |
| CSP syntactically valid | PASS | Semicolons correctly separate all directives. |

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
| `core:default` sufficient | PASS | All heavy lifting uses custom `#[tauri::command]` functions registered via `invoke_handler`. No Tauri plugins are used (no plugin features in `Cargo.toml`). File dialogs use `rfd` (native, bypasses Tauri ACL). Process spawning uses `std::process::Command`. |
| No Tauri plugins needing permissions | PASS | `tauri = { version = "2", features = [] }` — no plugin features enabled. |

### 1b. WebView2 / Platform-Specific

| Check | Status | Notes |
|-------|--------|-------|
| `set_dark_window_border` for Windows | PASS | Correctly uses DWM APIs in `setup()`. |
| `--autoplay-policy` browser arg | **FLAGGED** | Not set. WebView2 may block auto-playing audio (LiveKit's `RoomAudioRenderer`). Flagged as `AUDIT-TAURI` in `main.rs` setup closure. |
| `PermissionRequested` handler for getUserMedia | **FLAGGED** | Not implemented. WebView2 may silently deny camera/mic without this. Flagged as `AUDIT-TAURI` in `main.rs` setup closure. |
| Linux WebKitGTK minimum version | NOT DOCUMENTED | CI installs `libwebkit2gtk-4.1-dev`. WebRTC support varies by version. |
| PipeWire for Linux screen sharing | NOT DOCUMENTED | Required on Wayland. No detection/documentation. |

---

## Phase 2: WebRTC / LiveKit Audit

### 2a. Media Permissions Flow

**Room connection path:** User clicks Join Call -> `VoicePanel.handleJoin()` -> `useVoiceStore.joinCall()` -> `api.joinVoice()` (HTTP POST) -> receives `{ token, url }` -> `<LiveKitRoom serverUrl={url} token={token} audio={true}>` renders.

| Check | Status | Notes |
|-------|--------|-------|
| getUserMedia wrapped in try/catch | PASS | `AudioSettings.enumerateMediaDevices()` has try/catch. LiveKit SDK handles its own media errors internally. |
| Error feedback to user | PASS | `VoicePanel` shows `lastJoinError` from the store. `AudioSettings` shows "Grant microphone/camera access" hint when permission denied. |
| getDisplayMedia from user gesture | PASS | `MediaControls.toggleScreen()` is called from button `onClick` handler (direct user interaction). |
| Video elements have `autoplay`/`playsinline` | PASS | Handled by `@livekit/components-react`'s `VideoTrack` component internally. Message video elements use `playsInline` explicitly. |
| Remote tracks rendered via `TrackSubscribed` | PASS | `useTracks()` and `useParticipants()` from `@livekit/components-react` handle this. |
| Race condition: DOM vs track arrival | PASS | LiveKit React components handle this internally via hooks. |

### 2b. LiveKit Connection Configuration

| Check | Status | Notes |
|-------|--------|-------|
| LiveKit URL source (Tauri host mode) | PASS | `start_local_livekit` sets `ANNEX_LIVEKIT_PUBLIC_URL` env var before server starts. `joinVoice` API returns URL from `voice_service.get_public_url()`. |
| API key/secret auto-generated | PASS | `uuid::Uuid::new_v4()` generates random key and secret per launch. |
| LiveKit server auto-started | PASS | `StartupModeSelector.applyHost()` calls `startLocalLiveKit()` before `startEmbeddedServer()`. |
| WebSocket URL scheme (ws:// vs wss://) | **NOTED** | LiveKit URL is `ws://127.0.0.1:7880`. From `https://tauri.localhost` context, this is technically mixed content. Chromium treats 127.0.0.1 as "potentially trustworthy" so it works. Flagged as `AUDIT-TAURI` in `ws.ts` for Linux WebKitGTK testing. |
| STUN/TURN servers | NOT CONFIGURED | No `iceServers` configuration found. LiveKit uses its own ICE/TURN infrastructure. For local-only use (Tauri host mode), WebRTC peer connections are on localhost so STUN is not needed. For remote clients via tunnel, this is an inherent limitation. |

### 2c. LiveKit Port

| Check | Status | Notes |
|-------|--------|-------|
| Port 7880 hardcoded | **FLAGGED** | `start_local_livekit` uses `let port: u16 = 7880` with no fallback. If port is in use, livekit-server will fail to bind and the 15-second timeout will fire. Flagged as `AUDIT-TAURI` in `main.rs`. |

---

## Phase 3: State Management and IPC Audit

### 3a. Tauri Commands

All `#[tauri::command]` functions in `main.rs` with their registration status:

| Command | Registered | Frontend Caller | Serialization |
|---------|-----------|----------------|---------------|
| `get_startup_mode` | YES | `tauri.ts:getStartupMode()` | `Option<StartupPrefs>` — OK |
| `save_startup_mode` | YES | `tauri.ts:saveStartupMode()` | `StartupPrefs` — OK |
| `clear_startup_mode` | YES | `tauri.ts:clearStartupMode()` | `Result<(), String>` — OK |
| `start_embedded_server` | YES | `tauri.ts:startEmbeddedServer()` | `Result<String, String>` — OK |
| `start_tunnel` | YES | `tauri.ts:startTunnel()` | `Result<String, String>` — OK |
| `stop_tunnel` | YES | `tauri.ts:stopTunnel()` | `Result<(), String>` — OK |
| `get_tunnel_url` | YES | `tauri.ts:getTunnelUrl()` | `Option<String>` — OK |
| `export_identity_json` | YES | `tauri.ts:exportIdentityJson()` | `Result<Option<String>, String>` — OK |
| `get_livekit_config` | YES | `tauri.ts:getLiveKitConfig()` | `LiveKitSettingsResponse` — OK |
| `save_livekit_config` | YES | *Not called from frontend* | Backend-only (retained for future use) |
| `clear_livekit_config` | YES | *Not called from frontend* | Backend-only (retained for future use) |
| `check_livekit_reachable` | YES | *Not called from frontend* | Backend-only (retained for future use) |
| `start_local_livekit` | YES | `tauri.ts:startLocalLiveKit()` | `Result<serde_json::Value, String>` — OK |
| `stop_local_livekit` | YES | *Not called from frontend* | Backend-only (retained for future use) |
| `get_local_livekit_url` | YES | *Not called from frontend* | Backend-only (retained for future use) |

**Note:** `save_livekit_config`, `clear_livekit_config`, `check_livekit_reachable`, `stop_local_livekit`, and `get_local_livekit_url` are registered but not called from the frontend since the LiveKit settings panel was removed and auto-configuration was added. They remain functional as backend APIs (no dead code in the Rust binary — they're compiled and registered).

### 3b. Event System

| Check | Status | Notes |
|-------|--------|-------|
| Tauri events used | NO | No `app.emit()` / `window.emit()` in Rust. No `listen()` / `once()` in frontend. All communication is request-response via `invoke()`. No event name mismatch risk. |

### 3c. Docker vs Tauri State Paths

| State | Docker Source | Tauri Source | Path Implemented |
|-------|-------------|-------------|-----------------|
| Identity keys | IndexedDB | IndexedDB | Same path |
| Server URL | Current origin (empty `_apiBaseUrl`) | `startEmbeddedServer()` return value | Divergent — correctly handled in `StartupModeSelector` |
| Startup prefs | localStorage | Tauri IPC (`startup_prefs.json` on disk) | Divergent — correctly handled with `isTauri()` guard |
| Audio settings | localStorage | localStorage | Same path |
| WebSocket | Same-origin relative URL | Absolute URL from `_apiBaseUrl` | Divergent — correctly handled in `ws.ts` |
| LiveKit token/URL | Server API response | Server API response | Same path (embedded server returns local URL) |

### 3d. Tauri Detection

| Check | Status | Notes |
|-------|--------|-------|
| `isTauri()` function | PASS | Uses `'__TAURI_INTERNALS__' in window` — correct for Tauri v2. |
| Consistently guarded | PASS | Used in `App.tsx`, `StartupModeSelector.tsx`, `StatusBar.tsx`. All Tauri-specific code paths are lazy-imported behind the guard. |

### 3e. Input Handling

| Check | Status | Notes |
|-------|--------|-------|
| No global shortcuts registered | PASS | No `globalShortcut` references found. |
| No `contenteditable` elements | PASS | All inputs use standard `<input>` and `<textarea>` elements. |
| No custom event handlers conflicting with Tauri | PASS | Standard React event handlers only. |

---

## Phase 4: Silent Auto-Configuration Audit

### 4a. LiveKit Server Configuration

| Check | Status | Notes |
|-------|--------|-------|
| Auto-started with zero user interaction | PASS | `StartupModeSelector.applyHost()` calls `startLocalLiveKit()` automatically. |
| API key/secret auto-generated | PASS | `uuid::Uuid::new_v4()` per launch — not hardcoded. |
| Credentials stored in env vars | PASS | Set via `set_var` before server starts. Not persisted to disk (regenerated each launch). |
| Port auto-selected on collision | **FAIL** | Port 7880 is hardcoded. See Phase 2. |
| User feedback on failure | PARTIAL | `startLocalLiveKit` returns an error that surfaces in `StartupModeSelector` as an error phase. However, the error message ("livekit-server startup timed out") could be clearer about port conflicts. |

### 4b. Network Configuration

| Check | Status | Notes |
|-------|--------|-------|
| NAT detection | NOT IMPLEMENTED | Inherent limitation — LiveKit handles ICE internally. |
| STUN/TURN defaults | NONE | LiveKit dev mode doesn't configure external STUN/TURN. |
| Graceful degradation | PARTIAL | Voice join failures show error messages. Text continues working. No explicit "text-only mode" fallback. |

### 4c. Device Selection

| Check | Status | Notes |
|-------|--------|-------|
| Devices auto-selected | PASS | System default used when `inputDeviceId`/`outputDeviceId` are null. |
| Hot-plug detection | NOT IMPLEMENTED | `enumerateDevices()` only called when AudioSettings dialog opens. No `devicechange` event listener. |
| Graceful failure on unavailable device | PASS | LiveKit SDK falls back to default device. |

### 4d. Remaining Settings Panels

| Component | Purpose | Status |
|-----------|---------|--------|
| `AudioSettings.tsx` | Device/volume selection | NEEDED — user-facing |
| `IdentitySettings.tsx` | Persona management | NEEDED — user-facing |
| `UsernameSettings.tsx` | Display name management | NEEDED — user-facing |
| `AdminPanel.tsx` (Server Settings) | Server configuration | NEEDED — admin-facing |
| `AdminPanel.tsx` (Policy Editor) | Server policy | NEEDED — admin-facing |

**Dead references to deleted LiveKit settings panel:** NONE found. No imports, routes, or navigation links point to a deleted `LiveKitSettings` component. The file never existed as a standalone component — the previous commit's description was misleading. What was actually removed was LiveKit configuration UI that was inline in the AdminPanel.

---

## Phase 5: Build and Bundle Verification

### 5a. Build Command

| Check | Status | Notes |
|-------|--------|-------|
| `beforeBuildCommand` path | **FIXED** | Was `node ../scripts/build-desktop.js` (resolves to `crates/scripts/build-desktop.js` — doesn't exist). Fixed to `node ../../scripts/build-desktop.js`. This was a **build-breaking bug** — `cargo tauri build` would fail immediately. |
| `beforeDevCommand` path | PASS | Correctly uses `../../scripts/` and `../../client`. |
| `frontendDist` path | PASS | `../../client/dist` — correct relative path. |
| Build order | PASS | `build-desktop.js` runs ZK build, copies assets, then runs `npm run build` (which does `tsc -b && vite build`). Frontend is built before Tauri bundles it. |

### 5b. Asset Bundling

| Check | Status | Notes |
|-------|--------|-------|
| Bundle resources | PASS | `membership_vkey.json`, `assets/piper`, `assets/voices` are included. |
| Source maps excluded | **FIXED** | Was `sourcemap: true` in `vite.config.ts`. Changed to `false`. Source maps should not ship in production — they leak source code and bloat the bundle. |

### 5c. Dev vs Prod Configuration

| Check | Status | Notes |
|-------|--------|-------|
| `devUrl` only used in dev | PASS | `http://localhost:5173` — only active during `cargo tauri dev`. |
| No localhost URLs in production | PASS | Frontend uses `_apiBaseUrl` set at runtime. Vite proxy config is dev-only. |
| Console logging | NOT STRIPPED | `console.log`, `console.warn`, `console.error` present but gated behind meaningful conditions. Acceptable. |

---

## Phase 6: Previous Fix Verification

### Fix 1: LiveKit settings panel deleted

| Check | Status | Notes |
|-------|--------|-------|
| `LiveKitSettings.tsx` deleted | N/A | File never existed as a standalone component. |
| No dead imports/references | PASS | No imports of `LiveKitSettings` found in the entire codebase. |
| AdminPanel still functional | PASS | Has four sections: Server Settings, Policy Editor, Member Management, Channel Management. All render correctly. Policy editor has `voice_enabled` toggle. |
| Backend commands retained | PASS | `get_livekit_config`, `save_livekit_config`, `clear_livekit_config` remain registered and functional. |

### Fix 2: `useHttpsScheme: true`

| Check | Status | Notes |
|-------|--------|-------|
| Present in config | PASS | `tauri.conf.json` line 23: `"useHttpsScheme": true` |
| No `http://tauri.localhost` fetch targets | PASS | Only in CORS allowed_origins (fallback, not a client-side URL). |
| LiveKit WS endpoint | NOTED | Uses `ws://127.0.0.1:7880` — technically mixed content from `https://tauri.localhost`, but Chromium treats 127.0.0.1 as trustworthy. See Phase 2 notes. |

### Fix 3: CSP `media-src` added

| Check | Status | Notes |
|-------|--------|-------|
| `media-src` present | PASS | Includes `'self' blob: data: mediastream: http: https:` |
| `connect-src` allows WebSocket | PASS | `* ws: wss:` |
| `worker-src` allows blob | PASS | `'self' blob:` |
| CSP syntactically valid | PASS | All semicolons present. |

### Fix 4: Success message color

SKIPPED (cosmetic).

---

## Issues Fixed in This Audit

| # | Severity | File | Issue | Fix |
|---|----------|------|-------|-----|
| 1 | **CRITICAL** | `tauri.conf.json` | `beforeBuildCommand` path `../scripts/build-desktop.js` resolves to `crates/scripts/build-desktop.js` (doesn't exist). Build would fail. | Changed to `../../scripts/build-desktop.js` |
| 2 | **HIGH** | `vite.config.ts` | `sourcemap: true` ships source maps in production Tauri bundle. Leaks source code, bloats bundle. | Changed to `sourcemap: false` |
| 3 | **MEDIUM** | `tauri.conf.json` | CSP missing `font-src` directive. Falls back to `default-src` which doesn't include `data:`. CSS dependencies using data: URI fonts would be blocked. | Added `font-src 'self' data:` |

---

## Issues Requiring Hardware Test

All flagged with `// AUDIT-TAURI:` in the codebase.

| # | Platform | File | Issue |
|---|----------|------|-------|
| 1 | Windows | `main.rs` setup closure | WebView2 may silently deny `getUserMedia` without a `PermissionRequested` event handler. Test mic/camera access on Windows. |
| 2 | Windows | `main.rs` setup closure | WebView2 autoplay policy may block `RoomAudioRenderer` (remote audio in voice calls). May need `--autoplay-policy=no-user-gesture-required` via `additional_browser_args`. |
| 3 | Linux | `ws.ts` | WebKitGTK may enforce stricter mixed-content rules than Chromium. `ws://127.0.0.1` from `https://tauri.localhost` may be blocked. Test WebSocket connection on Linux. |
| 4 | Linux | `VoicePanel.tsx` | PipeWire is required for screen sharing on Wayland. Not detected or documented. |
| 5 | All | `main.rs` | LiveKit port 7880 is hardcoded. Will fail if port is in use. Test with another process on 7880. |
| 6 | All | `AudioSettings.tsx` | `getUserMedia` behavior in Tauri webview — verify device enumeration works and permission dialogs appear. |

---

## Dead Code / Dead References

| Item | Status | Notes |
|------|--------|-------|
| `save_livekit_config` command | ALIVE (backend) | Registered in invoke_handler, functional, but no frontend caller. Retained for potential future admin CLI or re-added settings UI. Not dead — compiled and reachable via `invoke()`. |
| `clear_livekit_config` command | ALIVE (backend) | Same as above. |
| `check_livekit_reachable` command | ALIVE (backend) | Same as above. |
| `stop_local_livekit` command | ALIVE (backend) | Same as above. |
| `get_local_livekit_url` command | ALIVE (backend) | Same as above. |
| LiveKit settings UI imports | NONE | No orphaned imports found. |

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

---

## Tauri Permissions Final State

**Capabilities file:** `crates/annex-desktop/capabilities/default.json`

```json
{
  "identifier": "default",
  "description": "Default capability set for the Annex desktop application.",
  "windows": ["main"],
  "permissions": ["core:default"]
}
```

`core:default` is sufficient because:
- All app functionality uses custom `#[tauri::command]` functions (auto-allowed with `core:default`)
- No Tauri plugins are used (`tauri` has no `features` enabled)
- File dialogs use `rfd` crate (native, no Tauri ACL needed)
- Process spawning uses `std::process::Command` (no Tauri shell plugin needed)
- Keyring access uses `keyring` crate (native, no Tauri ACL needed)

---

## Remaining Risk Areas

### HIGH RISK
1. **Windows WebView2 getUserMedia** — Without a `PermissionRequested` handler, camera and microphone access may silently fail on Windows. This is the single most likely cause of "voice doesn't work in Tauri on Windows" reports. The LiveKit SDK would connect, but publish silent/blank tracks.

2. **WebView2 autoplay policy** — Remote audio from LiveKit (`RoomAudioRenderer`) may not play if WebView2 blocks autoplay. Users would see participants but hear nothing.

### MEDIUM RISK
3. **LiveKit port collision** — Port 7880 hardcoded. Second Annex instance or other software on the same port causes a 15-second timeout followed by a cryptic error.

4. **Linux WebKitGTK mixed content** — `ws://127.0.0.1` from `https://tauri.localhost` may be blocked depending on WebKitGTK version and configuration.

### LOW RISK
5. **No hot-plug device detection** — Plugging in a USB mic mid-call won't auto-switch or notify. User must re-open AudioSettings.

6. **Remote LiveKit access via tunnel** — LiveKit runs on `ws://127.0.0.1:7880`, which is unreachable from remote clients connecting via the cloudflared tunnel. Remote users can chat (text) but cannot join voice calls. This is an architectural limitation, not a bug.

7. **STUN/TURN not configured** — For local-only use, this is fine. For any non-localhost WebRTC (which the app doesn't currently support in Tauri mode), STUN/TURN would be required.
