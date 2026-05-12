# Annex Desktop: Production Requirements

This is the operating manual for the Annex desktop app — a Tauri 2 shell
(`crates/annex-desktop/`) that wraps the same Axum server (`annex-server`)
either embedded in-process (Host mode) or proxies to a remote (Client mode).
Windows + Linux are release-critical (`I-DESKTOP-1`); macOS is deferred but
not deleted (`I-DESKTOP-2`).

## Tauri shell layout

| Path                                          | Purpose                                                                |
| --------------------------------------------- | ---------------------------------------------------------------------- |
| `crates/annex-desktop/src/main.rs`            | Entrypoint; Tauri commands; embedded server lifecycle.                 |
| `crates/annex-desktop/build.rs`               | Tauri build hook + WebKit version warning (Linux).                     |
| `crates/annex-desktop/tauri.conf.json`        | Tauri 2 config — bundle, windows, security CSP, plugins.               |
| `crates/annex-desktop/Entitlements.plist`     | macOS sandbox entitlements (deferred but kept).                        |
| `crates/annex-desktop/Info.plist`             | macOS Info.plist (deferred but kept).                                  |
| `crates/annex-desktop/icons/`                 | Bundled icons (32×32, 128×128, 128×128@2x, .icns, .ico).               |
| `crates/annex-desktop/nsis/hooks.nsi`         | Windows NSIS installer hooks.                                          |
| `crates/annex-desktop/capabilities/`          | Tauri 2 capabilities (allowlists for IPC commands).                    |
| `crates/annex-desktop/dist/`                  | Frontend build output, copied here by `scripts/build-desktop.js`.      |

## Windows requirements

### Build environment

- **Windows runner** (CI: `windows-latest`).
- **MSVC C++ toolchain** — Visual Studio with `Microsoft.VisualStudio.Workload.NativeDesktop`. Verified by `release-desktop.yml::Verify Visual Studio C++ workload`.
- **Rust toolchain pinned** to `rust-toolchain.toml` (channel `1.88`) — `dtolnay/rust-toolchain@1.88` in `release-desktop.yml`.
- **Node.js 22** — for `scripts/build-desktop.js` and the ZK setup.
- **CMake env knob**: `CMAKE_ARGS: -DCMAKE_POLICY_VERSION_MINIMUM=3.5` is set in CI; without it, an upstream cmake-using crate fails to configure on the modern CMake shipped on `windows-latest`.

### Runtime requirements (end-user)

- **WebView2 runtime.** `tauri.conf.json::bundle::windows::webviewInstallMode = "downloadBootstrapper"` ships a small bootstrapper that downloads WebView2 silently if the user doesn't already have it. Do NOT switch this to `embedBootstrapper` without explicit task scope (it inflates the installer significantly).
- **NSIS installer** with both per-user and per-machine modes (`installMode: "both"`). User chooses at install time.

### Bundle outputs

- `target/x86_64-pc-windows-msvc/release/bundle/nsis/*.exe` — NSIS installer.

### Common Windows breakage

- **Backslash paths in `config.toml`.** `crates/annex-desktop/src/main.rs::ensure_config` writes paths with forward slashes; `fix_backslash_paths` rewrites a config that contains `:\\` patterns. Don't reintroduce the original double-quoted backslash form — TOML parses `\\U` as a unicode escape and corrupts the path.
- **`\\?\` extended-length paths.** Tauri on Windows uses these and they don't follow NTFS junctions. `scripts/build-desktop.js` deliberately copies `client/dist` into `crates/annex-desktop/dist` (rather than using a junction) to avoid this. Don't replace the copy with a junction.
- **Code signing.** Currently `TAURI_SIGNING_PRIVATE_KEY: ""` in `release-desktop.yml`; signed-installer support is a future task. Until then expect SmartScreen warnings on first install.

## Linux requirements

### Build environment

- **Ubuntu runner** (CI: `ubuntu-latest` for `ci.yml::check-desktop`, `ubuntu-22.04` for `release-desktop.yml::build`). Use `ubuntu-22.04` for releases — that's what we test against.
- **System packages** (matches what `scripts/claude-setup.sh` installs):
  - `libwebkit2gtk-4.1-dev` (and runtime `libwebkit2gtk-4.1-0`) — provides `webkit2gtk-4.1.pc`. **NOT `webkitgtk-4.1`** — that pkg-config name does not exist for this dev package.
  - `libgtk-3-dev`
  - `libsoup-3.0-dev`
  - `javascriptcoregtk-4.1-dev`
  - `libpipewire-0.3-dev`
  - `libappindicator3-dev`, `librsvg2-dev`, `patchelf`
  - `libxdg-desktop-portal-dev` (release matrix only; needed for portal-based file/picker dialogs)
- **WebKitGTK ≥ 2.40.** WebRTC features `getUserMedia()` and `getDisplayMedia()` are gated on this. Verified by `release-desktop.yml::Verify WebKitGTK version` and the soft check in `crates/annex-desktop/build.rs`. The pkg-config name to query is `webkit2gtk-4.1`; using `webkitgtk-4.1` (no "2") will silently fail the check on this dev package.
- **PipeWire** — required for screen capture / audio capture on modern Linux.

### Runtime requirements (end-user)

- DEB control file declares (`tauri.conf.json::bundle::linux::deb::depends`):
  - `libwebkit2gtk-4.1-0 (>= 2.40)`
  - `libgtk-3-0`
  - `libpipewire-0.3-0`
- AppImage embeds these libs and is largely portable across distros >= 22.04 era.

### Bundle outputs

- `target/x86_64-unknown-linux-gnu/release/bundle/deb/*.deb`
- `target/x86_64-unknown-linux-gnu/release/bundle/appimage/*.AppImage`

### Common Linux breakage

- **WebKitGTK too old.** Distros earlier than Ubuntu 22.04 ship WebKitGTK < 2.40 and lose `getUserMedia`. Voice / screen share will fail at runtime with cryptic errors. The release-desktop.yml gate enforces ≥ 2.40 at CI time.
- **Wayland portal missing.** If `libxdg-desktop-portal` isn't present at runtime, screen-share dialogs return empty. Document the dependency in user-facing release notes.
- **PulseAudio vs PipeWire.** Modern desktops use PipeWire; the older `pulseaudio` symlinks usually work, but distros that aliased away the symlinks (some minimal installs) surface a mic-not-found error on first voice join.

## macOS (deferred)

The macOS matrix entries in `release-desktop.yml` (`aarch64-apple-darwin`,
`x86_64-apple-darwin`) and `Build (macOS)` in `ci.yml` are kept even when
macOS is not the immediate release target. `tauri.conf.json::bundle::macOS`
remains intact:

- `entitlements: "Entitlements.plist"`
- `infoPlist: "Info.plist"`
- `minimumSystemVersion: "11.0"`

Reasons not to delete:
- Re-enabling later requires re-deriving entitlements and provisioning profile metadata. Keeping them preserves that history.
- A passing macOS CI lane (even if not blocking) catches regressions early.

## Tauri resource requirements

`tauri.conf.json::bundle::resources` (paths are relative to
`crates/annex-desktop/tauri.conf.json`, NOT to repo root):

- `../../zk/keys/membership_vkey.json` — Groth16 verification key. Required at server boot. Empty / dummy file means every proof rejection at runtime. See `zk-merkle-production.md`.
- `../../assets/piper` — Piper TTS binary (`piper` on Linux/macOS, `piper.exe` on Windows). Populated by `scripts/setup-piper.sh` / `setup-piper.ps1`.
- `../../assets/voices` — Piper voice models (e.g. `en_US-lessac-medium.onnx` + `*.onnx.json`). Populated by the same setup scripts.

If any of these resources is missing or zero-byte at bundle time:
- Tauri will fail to bundle (Linux/macOS resource resolution checks file existence).
- On Windows, an empty file will be silently bundled and break at runtime.

`scripts/build-desktop.js` exits with code 1 if `assets/piper/piper(.exe)` or
the voice `.onnx` is missing after `setup-piper.*` runs. **Don't bypass that
check.**

### Bundle path resolution caveat

`tauri.conf.json::beforeBuildCommand = "node ../scripts/build-desktop.js"`
relies on Tauri 2's `app_dir` heuristic: starting at `tauri-dir`
(`crates/annex-desktop/`), Tauri walks up looking for the nearest
`package.json`; with none in `crates/annex-desktop/` or any parent before the
repo root, the heuristic falls back to `tauri_dir.parent()` (i.e.
`crates/`). From `crates/`, `../scripts/build-desktop.js` correctly resolves
to `<repo-root>/scripts/build-desktop.js`.

If a `package.json` is ever added to `crates/annex-desktop/` (or any
intermediate dir), the cwd flips and the path breaks. **Either keep the
parent layout package-json-free, or migrate the path to `../../scripts/...`
and `../../client` for determinism.**

## Embedded server lifecycle

Source: `crates/annex-desktop/src/main.rs`.

1. **Startup mode resolution.**
   - `get_startup_mode` reads `<data_dir>/startup_prefs.json`.
   - First run: file absent → frontend shows `StartupModeSelector`.
   - Subsequent runs: file present → app proceeds in saved mode.
2. **Host mode.**
   - `start_embedded_server` is a Tauri command that's idempotent. If a server is already running it returns the cached URL. Otherwise:
     - Resolves `<data_dir>/config.toml` (writes a default if absent; fixes Windows backslashes if present).
     - Calls `annex_server::config::load_config(Some(path))`.
     - Calls `annex_server::init_tracing` (no-op on second call).
     - Calls `annex_server::prepare_server(cfg)` to get `(listener, router)`.
     - Stores the URL in `AppManagedState::server`.
     - Spawns the Axum task via `tauri::async_runtime::spawn`.
3. **Client mode.**
   - No embedded server. The webview is pointed at the user-supplied remote URL.
4. **Router session.**
   - `RouterSessionState::public_url` carries the publicly-reachable HTTPS endpoint allocated by an Annex routing layer (replacement for the older cloudflared tunnel). Released on shutdown via `session_id`.
5. **Reset.**
   - `reset_server_data` removes `<data_dir>/{annex.db, annex.db-wal, annex.db-shm, uploads/, config.toml}`. Refused while the server is running. The first-run marker `<data_dir>/first_run_completed` is **not** cleared by reset — that's intentional (logout + relaunch should not nuke server data).
6. **Shutdown.**
   - On window close, the spawned Axum task is dropped. SQLite WAL is flushed on `Drop` of the pool. The `RouterSessionState` is released via `session_id`.

## ZK asset inclusion

- `zk/keys/membership_vkey.json` is bundled (see resources above) and read at boot via `crates/annex-server/src/lib.rs`.
- `zk/build/membership_js/membership.wasm` and `zk/keys/membership_final.zkey` are NOT bundled into the desktop crate's resources. They're shipped via the frontend build:
  - `scripts/build-desktop.js` copies them from `zk/build/.../membership.wasm` and `zk/keys/membership_final.zkey` into `client/public/zk/`.
  - Vite emits them under `client/dist/assets/...` as part of the static frontend.
  - Loaded at runtime by the proof worker: `client/src/workers/proof.worker-*.ts` (compiled into `dist/assets/proof.worker-*.js`).

If the server starts with a dummy vkey but the client has a real proving
key, every proof will be generated correctly and rejected at the server. The
runtime warning in `lib.rs` (the "ZK verification key not found" log line) is
the one signal users will have. Treat its presence in production logs as a
build-misconfiguration alert, not noise.

## WebRTC / media caveats

- The WebRTC media plane is hosted in-process by the Axum server (via `webrtc-rs`). LiveKit references in older docs are stale.
- Voice config knobs live under `[webrtc]` in `config.toml`. STUN defaults to Google STUN; for restrictive networks add a TURN entry.
- `getUserMedia` / `getDisplayMedia` require WebKitGTK ≥ 2.40 (Linux). On Windows the WebView2 runtime handles media; on macOS, WKWebView's stack handles it. There is **no fallback path**; an old WebKitGTK is a hard runtime failure.
- The Tauri CSP at `tauri.conf.json::app::security::csp` allows `connect-src * ws: wss:` and `media-src 'self' blob: data: mediastream: http: https:` deliberately so that media streams can flow. Don't tighten this without testing voice end-to-end.

## Keyring behaviour

- Crate: `keyring = "3"` (dependency listed in `crates/annex-desktop/Cargo.toml`).
- Used for storing the user's signing key and other secrets keyed by `service = "Annex"` and a per-machine identifier.
- Backends:
  - **macOS** — Keychain (`security` framework). Requires user authentication for first access.
  - **Windows** — Credential Manager.
  - **Linux** — DBus Secret Service via `dbus-secret-service`. Requires a running secret-service provider (gnome-keyring, kwallet, or seahorse). On a headless / minimal Linux install this is **not present** by default; the keyring crate returns an error and the app falls back to a file-based store (read the code path under `resolve_signing_key`).
- Cleanup: `reset_server_data` does NOT clear keyring entries. A user wanting full reset must explicitly remove the entry via OS tooling. Document this in user-facing release notes.

## Local dev shortcuts

- `cargo tauri dev` (run from `crates/annex-desktop/`) — uses `beforeDevCommand: "node ../scripts/prepare-zk-dev.js && npm --prefix ../client run dev"`. Same `app_dir` resolution caveat applies.
- `prepare-zk-dev.js` ensures `client/public/zk/` is populated before Vite serves; if `zk/build/...` and `zk/keys/...` are missing it triggers the full `build-circuits.js` + `setup-groth16.js` flow.
- For a server-only loop (no Tauri), use `cargo run -p annex-server` against `config.toml` at the repo root and load the dev frontend from `npm --prefix client run dev` on `:5173`.
