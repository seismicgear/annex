# Annex Release Gates

A "gate" is a checkable condition. A change is mergeable when **all** gates
relevant to the surface it touches are green. A `v*` tag is cuttable when
**all** gates are green. Each gate names the exact command, the file it
lives in, and the failure modes it catches.

This file is intentionally redundant with `.github/workflows/ci.yml` and
`.github/workflows/release-desktop.yml`. If they drift, treat the workflow
as authoritative for CI and update this file.

## Gate matrix

| Surface           | Gate names                                                                                  |
| ----------------- | ------------------------------------------------------------------------------------------- |
| Server            | `srv-fmt`, `srv-clippy`, `srv-test`, `srv-zk-keys-present`                                  |
| Linux desktop     | `lin-syslibs`, `lin-build`, `lin-bundle-deb`, `lin-bundle-appimage`                         |
| Windows desktop   | `win-vc`, `win-build`, `win-bundle-nsis`                                                    |
| Frontend          | `fe-deps`, `fe-lint`, `fe-test`, `fe-build`                                                 |
| ZK artifacts      | `zk-deps`, `zk-circuit`, `zk-setup`, `zk-proof`, `zk-vkey-shipped`                          |
| Migrations        | `mig-numbered`, `mig-no-edit`, `mig-applies`                                                |
| Smoke / E2E       | `e2e-server-up`, `e2e-startup-flow`, `e2e-no-console-errors`, `smoke-server`, `smoke-desktop-build` |
| UI audit          | `ui-audit-surfaces`, `ui-audit-baselines`, `ui-audit-a11y`                                    |

---

## Server gates

### srv-fmt
- Command: `cargo fmt --all --check`
- Workflow: `.github/workflows/ci.yml::check-server::cargo fmt`
- Catches: import sort drift, indent drift.
- Pinned by: `rust-toolchain.toml` (channel 1.88).

### srv-clippy
- Command: `cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings`
- Workflow: `.github/workflows/ci.yml::check-server::cargo clippy`
- Catches: lints with the project's deny-warnings policy. The `-D warnings` is non-negotiable; PRs that introduce a clippy warning must fix it, not silence it.

### srv-test
- Command: `cargo test --workspace --exclude annex-desktop --no-fail-fast`
- Workflow: `.github/workflows/ci.yml::check-server::cargo test`
- Catches: lib + integration test regressions. `--no-fail-fast` is required so the full inventory is reported instead of bailing on the first crate failure.
- Local note: tests use in-memory SQLite (`:memory:`) via `tests/common/mod.rs::setup_test_app`. Some WS tests bind a real `TcpListener` on `127.0.0.1:0`.

### srv-zk-keys-present
- Pre-test step (CI): `(cd zk && npm ci && node scripts/build-circuits.js && node scripts/setup-groth16.js)`
- Workflow: `.github/workflows/ci.yml::check-server::Generate ZK keys`
- Catches: missing `zk/keys/membership_vkey.json`. Without it, `crates/annex-server/src/lib.rs` falls back to `generate_dummy_vkey()` and emits a warning. The dummy key is not acceptable for any release artifact (see I-ZK-2).

---

## Linux desktop gates

### lin-syslibs
- Command (Ubuntu 22.04): `apt-get install -y libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev javascriptcoregtk-4.1-dev libpipewire-0.3-dev libappindicator3-dev librsvg2-dev patchelf`
- Workflow: `.github/workflows/release-desktop.yml::build::Install Linux dependencies` and `.github/workflows/ci.yml::check-desktop::Install Linux dependencies`
- Catches: missing GTK3 / WebKitGTK / PipeWire dev packages. **Verify the WebKitGTK version with `pkg-config --modversion webkit2gtk-4.1`**. Do not use `webkitgtk-4.1` (no "2") — only `webkit2gtk-4.1.pc` ships with `libwebkit2gtk-4.1-dev`.

### lin-build
- Command: `cargo build -p annex-desktop --release`
- Workflow: `.github/workflows/ci.yml::check-desktop::cargo build (release)`
- Catches: link-time and compile-time desktop breakages.

### lin-bundle-deb / lin-bundle-appimage
- Command: `cargo tauri build --target x86_64-unknown-linux-gnu` (run from `crates/annex-desktop/`)
- Workflow: `.github/workflows/release-desktop.yml::build::Build Tauri app` (Linux matrix entry)
- Catches: bundle-time failures (icon, NSIS-equivalent, `bundle::linux::deb::depends`, AppImage tooling). The `beforeBuildCommand` must succeed first — that runs `node ../scripts/build-desktop.js` (path resolves through Tauri's `app_dir` heuristic; see `desktop-production.md`).
- Output paths: `target/x86_64-unknown-linux-gnu/release/bundle/deb/*.deb`, `…/appimage/*.AppImage`.

---

## Windows desktop gates

### win-vc
- Command (CI): `vswhere.exe -latest -products * -requires Microsoft.VisualStudio.Workload.NativeDesktop -property installationPath`
- Workflow: `.github/workflows/release-desktop.yml::build::Verify Visual Studio C++ workload`
- Catches: missing MSVC build tools — annex-desktop and several Rust deps need a C++ toolchain.

### win-build
- Command: `cargo build -p annex-desktop --release`
- Workflow: `.github/workflows/ci.yml::build-windows::cargo build (release)`
- Env: `CMAKE_ARGS: -DCMAKE_POLICY_VERSION_MINIMUM=3.5` (required for an upstream cmake-using crate to build on the matrix).

### win-bundle-nsis
- Command: `cargo tauri build --target x86_64-pc-windows-msvc`
- Workflow: `.github/workflows/release-desktop.yml::build::Build Tauri app` (Windows matrix entry)
- Catches: NSIS bundle problems, code-signing setup issues. NSIS hooks live in `crates/annex-desktop/nsis/hooks.nsi`; `tauri.conf.json::bundle::windows::webviewInstallMode` must remain `downloadBootstrapper` so end users without WebView2 still get installed.
- Output: `target/x86_64-pc-windows-msvc/release/bundle/nsis/*.exe`.

---

## Frontend gates

### fe-deps
- Command: `npm --prefix client ci`
- Workflow: `.github/workflows/ci.yml::test-frontend::npm ci`
- Catches: `package-lock.json` drift, missing deps. **Use `ci`, not `install`** — `install` mutates the lockfile.

### fe-lint
- Command: `npm --prefix client run lint`
- Workflow: `.github/workflows/ci.yml::test-frontend::npm run lint`
- Catches: ESLint errors (eslint flat config at `client/eslint.config.js`; recommended TS/JS rules + react-hooks + react-refresh).
- Common breaks: `react-hooks/set-state-in-effect`, `@typescript-eslint/no-unused-vars`.

### fe-test
- Command: `npm --prefix client test -- --run`
- Workflow: `.github/workflows/ci.yml::test-frontend::npm test`
- Catches: Vitest unit + RTL component test regressions. 149 tests at last baseline.

### fe-build
- Command: `npm --prefix client run build`
- Workflow (implicit; runs as part of desktop bundle build): `tauri.conf.json::build::beforeBuildCommand → scripts/build-desktop.js → npm run build`
- Catches: TS errors (`tsc -b`) and Vite build issues.
- Known noise: a 3 MB `main.js` chunk warning + 3 dynamic-vs-static import collisions. Tracked but not blocking.

---

## ZK artifact gates

### zk-deps
- Command: `(cd zk && npm ci)`
- Workflow: every CI lane that needs ZK keys runs this first.
- Catches: stale snarkjs / circomlib pinning.

### zk-circuit
- Command: `(cd zk && node scripts/build-circuits.js)`
- Outputs: `zk/build/{identity,membership}.r1cs`, `…_js/{name}.wasm`.
- Catches: circom compile errors (e.g. unbound signal, wrong include path).

### zk-setup
- Command: `(cd zk && node scripts/setup-groth16.js)`
- Outputs: `zk/keys/pot14_*.ptau`, `{identity,membership}_0.zkey`, `{identity,membership}_final.zkey`, `{identity,membership}_vkey.json`.
- Catches: trusted-setup failures, missing entropy.
- Note: the script reuses `pot14_final.ptau` if it already exists; only the per-circuit zkey/vkey are regenerated. **Production-quality keys must come from a real ceremony** — see `zk-merkle-production.md`.

### zk-proof
- Command: `(cd zk && node scripts/test-proofs.js)`
- Catches: 16/16 must pass: identity validity, identity tampering rejection, identity input differentiation, membership validity for index 0 + 1, membership tamper rejection (proof, root, commitment), and the `mismatched leafIndex/pathIndexBits` rejection at witness generation time.

### zk-vkey-shipped
- Pre-bundle: `test -f zk/keys/membership_vkey.json` and **its content must be the result of a real `setup-groth16.js` run** (not the dummy emitted by the workflow's `|| true` fallback).
- Workflow: `.github/workflows/release-desktop.yml::build::Setup Node.js + Build Tauri app`. The `tauri.conf.json::bundle::resources` references `../../zk/keys/membership_vkey.json`; if the dummy is shipped, every bundled client will reject every real proof on startup.
- Failure mode: the `release-desktop.yml` script currently includes a `|| true` after the ZK setup step on Windows/macOS to keep the build moving in CI; **before tagging a release, confirm the ZK step actually succeeded by checking the produced vkey file's structure** (it should contain Groth16 protocol metadata).

---

## Migration gates

### mig-numbered
- Manual: every new SQL file in `crates/annex-db/src/migrations/` must use the next available number — no gaps, no reuse, no rebasing of an existing number.
- Catches: out-of-order migration application, accidental "downgrade".

### mig-no-edit
- Manual + git history: a previously-committed `crates/annex-db/src/migrations/NNN_*.sql` may not be modified, even for a comment-only fix. Some installations have already applied that file's content; a downstream checksum-based migration runner would diverge.
- Catches: silent breakage in upgrades from earlier deploys. See I-DB-1.

### mig-applies
- Command (covered by srv-test): startup of any `annex-server` or `annex-desktop` instance triggers `crates/annex-db/src/migrations.rs::apply_migrations` against a fresh in-memory SQLite. If migration N is malformed, server boot panics.
- Catches: SQL syntax errors, FK violations on default data, redundant indexes.

---

## Smoke / E2E gates

### e2e-server-up
- Command: `bash scripts/e2e-server.sh start`
- Catches: The script builds the client, places it under `client/dist`, then starts an Axum server on port 3000 with a fresh DB. If start fails, none of the E2E tests can run. Stop with `bash scripts/e2e-server.sh stop`.

### e2e-startup-flow
- Command: `cd client && npm run test:e2e` (Playwright Chromium headless against `http://127.0.0.1:3000`)
- Tests live in `client/e2e/` (e.g., `e2e/startup.spec.ts`). Each test gets a fresh browser context so IndexedDB is clean. The flow under test: IdentitySetup → StartupModeSelector → ZK proof → Chat UI.
- Failure artifacts: screenshots in `client/e2e-results/`, HTML report in `client/e2e-report/`.

### e2e-no-console-errors
- Manual / per-PR: when running locally against an embedded desktop server (`cargo tauri dev` from `crates/annex-desktop/`), no `[error]` lines from `tracing` and no console errors in the webview during the golden flow:
  1. Identity creation (offline, no network).
  2. Pick "Use this server".
  3. Send a message in default channel.
  4. Open voice; pick a channel; mute/unmute; leave.
  5. Reset (Tauri command `reset_server_data`); restart.

### smoke-server
- Linux command: `bash scripts/smoke-server.sh`
- Windows command: `pwsh scripts/smoke-server.ps1`
- Workflow: `.github/workflows/ci.yml::smoke-server-linux::Run server smoke` and `.github/workflows/ci.yml::smoke-server-windows::Run server smoke`
- What it covers: boots `annex-server` against a fresh temp data dir with `ANNEX_ENFORCE_ZK_PROOFS=true`, calls `/health`, runs the full register → Merkle path → Groth16 proof (via `snarkjs.groth16.fullProve` against `zk/keys/membership_final.zkey`) → `verify-membership` → authenticated `POST /api/channels` flow, then shuts the server down cleanly. The actual API calls live in `scripts/smoke-server-flow.mjs`; both shell wrappers stay thin.
- Artifact preconditions: `zk/keys/membership_vkey.json`, `zk/build/membership_js/membership.wasm`, `zk/keys/membership_final.zkey` must all exist as non-empty files (no dev fallback). The script exits with a clear error message if any are missing.
- Failure modes: server fails to bind / fails to reach `/health`; ZK artifacts missing or corrupt; proof verification rejected by `enforce_zk_proofs`; founder bootstrap regressed so `POST /api/channels` returns 403; binary leaks across runs (the script execs the built binary directly so the captured PID is the server itself, not the `cargo run` wrapper).
- Knobs: `ANNEX_SMOKE_PORT` (default `7321`), `ANNEX_SMOKE_HOST` (default `127.0.0.1`).

### ui-audit-surfaces
- Command: `bash scripts/ui-audit.sh`
- Workflow: `.github/workflows/ci.yml::ui-audit`
- Catches: a surface in `client/e2e/audit/surfaces.ts` that can no longer be reached — either the navigation recipe drifted from the UI, or the UI is broken. Also enforces manifest hygiene via `client/e2e/audit/manifest.spec.ts`: unique ids, known stages/roles/viewports, a non-empty `intent` per surface, a justified reason on every audit waiver, and — the important one — that every component rendering a `.dialog-overlay` is reached by some surface. A new dialog cannot silently go unaudited.
- Failure artifacts: `client/e2e/audit/diagnostics/<viewport>/<surface>.png` (screenshot of wherever the run ended up), uploaded by CI.

### ui-audit-baselines
- Command: same run; comparison is `toHaveScreenshot` against `client/e2e/audit/baselines/`.
- Catches: unintended visual drift, at a 0.5% pixel tolerance across four viewports (1440x900, 1280x800, 1024x768, 390x844). This is the guard that makes a CSS refactor safe: change a token, see exactly which screens moved.
- Updating: `bash scripts/ui-audit.sh --update-baselines`, committed separately and reviewed as a diff of images. Never update baselines in the same commit as the change that moved them without saying so.
- Note: baselines are recorded on Linux/Chromium. Font hinting differs enough across platforms that re-recording on macOS or Windows will produce spurious diffs — record on Linux.

### ui-audit-a11y
- Command: same run; axe-core (WCAG 2.1 A/AA + best-practice) per surface per viewport.
- Catches: missing accessible names, contrast failures, heading-order breaks, duplicate landmarks, and — via a separate check — dialogs that do not move focus in, do not trap it, or do not close on Escape.
- Findings are recorded to `docs/ui-audit/findings.json` rather than asserted, so the run completes and reports everything; the ledger is reviewed as part of the PR.

### smoke-desktop-build
- Linux command: `bash scripts/smoke-desktop-build.sh`
- Windows command: `pwsh scripts/smoke-desktop-build.ps1`
- Workflow: not currently a separate CI job — `lin-build` / `win-build` plus the Tauri bundle gates in `release-desktop.yml` are a strict superset. Use this script locally as a fast pass/fail before pushing.
- What it covers: verifies the three release-critical ZK artifacts are present and that `membership_vkey.json` parses as JSON; runs `npm --prefix client run build`; runs `cargo build -p annex-desktop --release`; confirms `client/dist/`, `client/public/zk/`, `zk/keys/membership_vkey.json`, and `target/release/annex-desktop[.exe]` exist as non-empty files.
- Dev-only knob: `SKIP_CLIENT_BUILD=1` (bash) / `-SkipClientBuild` (pwsh) skips the client build step. **Not for release / CI** — the script labels the branch dev-only and refuses to continue if `client/dist/index.html` is missing.

---

## Cutting a release

Order of operations to cut a `v*` tag:

1. All CI gates green on the merge commit.
2. ZK keys regenerated with documented entropy if there's a circuit change. Otherwise, retain existing keys; do not rotate casually.
3. `cargo build -p annex-desktop --release` succeeds locally on Linux **and** on a Windows machine (or via a manual `release-desktop.yml workflow_dispatch`).
4. `release-desktop.yml` matrix runs to completion; artifacts uploaded:
   - `annex-linux-x86_64` (`.deb`, `.AppImage`)
   - `annex-windows-x86_64` (`.exe`)
   - `annex-macos-arm64`, `annex-macos-x86_64` (`.dmg`) — deferred status acceptable; existence preferred.
5. Smoke-test each artifact on a clean VM: install, launch, run the e2e-no-console-errors flow.
6. Tag `vX.Y.Z`. The `release` job in `release-desktop.yml` will then assemble the GitHub Release draft.
