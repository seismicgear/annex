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
| ZK artifacts      | `zk-deps`, `zk-circuit`, `zk-setup`, `zk-proof`, `zk-vkey-shipped`, `zk-ceremony-verified`, `zk-production-gate` |
| Migrations        | `mig-numbered`, `mig-no-edit`, `mig-applies`                                                |
| Smoke / E2E       | `e2e-server-up`, `e2e-startup-flow`, `e2e-group-call`, `e2e-puppeteer`, `e2e-no-console-errors`, `smoke-server`, `smoke-federation`, `smoke-desktop-build` |
| UI audit          | `ui-audit-surfaces`, `ui-audit-baselines`, `ui-audit-a11y`, `ui-audit-order-independence`      |
| Desktop install   | `desktop-audit-package`                                                                       |
| Federation        | `signal-relay-contract`                                                                       |
| Documentation     | `roadmap-consistent`                                                                          |
| Supply chain      | `dep-deny`, `npm-audit-prod`, `npm-audit-report`, `codeql`                                     |
| Release pipeline  | `rel-preflight`, `rel-server-tarball`, `rel-assets`, `rel-notes`                               |

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
- Command: `cargo test --workspace --exclude annex-desktop`
- Workflow: `.github/workflows/ci.yml::check-server::cargo test`
- Catches: lib + integration test regressions. Add `--no-fail-fast` locally when you want the full inventory rather than a bail on the first crate failure; CI does not pass it, and this entry asserted that it did.
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
- Command: `cargo check -p annex-desktop` (CI) / `cargo build -p annex-desktop --release` (local pre-flight)
- Workflow: `.github/workflows/ci.yml::check-desktop-linux::cargo check (annex-desktop)`, then `cargo clippy (annex-desktop)`, then `Tauri build (debug — validates bundle wiring)`. The job name is `check-desktop-linux`, not `check-desktop`, and it never ran a release build.
- Catches: link-time and compile-time desktop breakages, and the bundle-resource validation the debug Tauri build performs.

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
- Command: `SKIP_PIPER=1 cargo tauri build --debug --bundles nsis`
- Workflow: `.github/workflows/ci.yml::check-desktop-windows::Tauri build (debug — validates bundle wiring)`. There is no `build-windows` job and CI runs no release build on Windows.
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
- Catches: Vitest unit + RTL component test regressions. 502 tests across 54 files at last baseline (`cd client && npm test`).
- Also typechecked, which it was not until recently: `ci.yml::test-frontend` runs `npx tsc -b` before `npm test`, and `client/tsconfig.json` references `tsconfig.test.json`, so `src/**/*.test.ts(x)` and `e2e/` are covered. Vitest transpiles with esbuild and cannot fail on a type error, so without that project a broken test file passed by being unparsed.

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
- Note: the script reuses `pot14_final.ptau` if it already exists; only the per-circuit zkey/vkey are regenerated. It is **dev-only** and refuses to run under a production profile. The production path is `zk/scripts/ceremony.js` — see `zk-ceremony-verified` below.

### zk-proof
- Command: `(cd zk && node scripts/test-proofs.js)`
- Workflow: `.github/workflows/ci.yml::check-server::ZK proof round-trip`, and `scripts/test-all.sh` (skipped there, with a stated reason, when `zk/keys` is empty).
- Ran in NO workflow until that step was added. CI's `ZK script tests` step runs `zk npm test`, which is `verify-artifacts.test.js` alone — it never generated or verified a proof. A gate this file described in detail was, for its whole life, a command nobody executed.
- Catches: identity validity, identity tampering rejection, identity input differentiation, membership validity for index 0 + 1, membership tamper rejection (proof, root, commitment), the `mismatched leafIndex/pathIndexBits` rejection at witness generation time, and the `membership_v2` assertions including the challenge binding. No count is quoted here on purpose — the previous "16/16" outlived three additions to the script.
- **It was broken for four commits and this file said 16/16 throughout.** `membership_v2.circom` gained `main {public [topicHash, challenge]}`; the script built its v2 witness with `topicHash` alone and asserted `publicSignals.length === 4` against a circuit producing five, so `fullProve` failed with "Only 45 out of 46 inputs set" in CI and in `test-all.sh`. Quoting a pass count in a document is not the same as running the command.

### zk-vkey-shipped
- Command: `ANNEX_BUILD_PROFILE=production node zk/scripts/verify-artifacts.js --all`
- Workflow: `.github/workflows/release-desktop.yml::build-{linux,windows,macos}::Verify pinned ZK artifacts`, and again inside `scripts/build-desktop.js` on the bundle path.
- Catches: a missing, dummy, or hash-mismatched verification key for ANY enabled circuit. `tauri.conf.json::bundle::resources` ships `../../zk/keys/membership_vkey.json`; a dummy there makes every bundled client reject every real proof.
- **`--all` is load-bearing.** A bare invocation defaults to the `membership` manifest alone, and the default identity path is `membership_v2`. The workflow ran the bare form until `scripts/verify-production-rejects-dev-fixtures.sh` was rewritten to check for it — five circuits of six were gated by nothing.
- This entry previously described a `|| true` after the ZK setup step on Windows/macOS. That fallback no longer exists in `release-desktop.yml`; the description outlived it and would have sent a reader looking for a hole that had been filled.

### zk-ceremony-verified
- Command: `node zk/scripts/verify-ceremony.js`
- Workflow: `.github/workflows/release-desktop.yml::build-*::Verify the ZK ceremony`, and inside `scripts/build-desktop.js`.
- Catches what hashes cannot. `verify-artifacts.js` proves the files are the pinned ones; it cannot prove a ceremony produced them, because a manifest and a matching set of files can both be written by anyone with commit access. This runs `snarkjs zkey verify` over r1cs → ptau → every contribution → beacon, re-derives the verification key from the proving key and compares it to the shipped one, and checks the transcript's drand round against what the League of Entropy actually published.
- `--offline` skips only the beacon check and says so in its summary rather than reporting a weaker check as the full one.

### zk-production-gate
- Command: `sh scripts/verify-production-rejects-dev-fixtures.sh`
- Workflow: `.github/workflows/ci.yml::check-server::Production ZK provenance gate` and `scripts/test-all.sh`.
- **That line was false until the step existed.** The globbed `Harness script tests` step runs `scripts/tests/*.test.sh`; this script lives in `scripts/` and is not named `*.test.sh`, so it matched neither the directory nor the suffix and ran in no workflow at all. `scripts/test-all.sh` did not call it either. Twelve assertions about whether a release can ship dev-fixture ZK keys, executed by hand when somebody remembered. The script now asserts that both callers invoke it, so this cannot come back quietly. 14 assertions.
- Tests the GATE, not the tree: it builds throwaway manifests in a temp directory and asserts each is refused with the right exit code — dev-fixture under production is exactly 3, an unknown ceremony type is 3, a ceremony claim with no transcript is 3, a named-but-absent transcript is 3, a tampered artifact is 2, and the same dev-fixture manifest under a dev profile is 0. Every invocation runs under `env -u ANNEX_ALLOW_DEV_CEREMONY`, because every expectation is meaningless if that bypass is set.
- Also asserts the release workflow runs `--all` and never sets the bypass. A gate nothing invokes is decoration; the previous version proved the script refuses and never checked that a release calls it.

---

## Supply-chain gates

An entire CI job was missing from this file, which claims to be "intentionally
redundant with ci.yml".

### dep-deny
- Command: `cargo deny --all-features check`
- Workflow: `.github/workflows/ci.yml::supply-chain::cargo deny`
- Catches: advisories, banned/duplicate crates, disallowed licences and
  unexpected sources across the whole Rust graph. Blocking — no
  `continue-on-error`.

### npm-audit-prod
- Command: `npm audit --omit=dev --audit-level=high` in `client/`
- Workflow: `.github/workflows/ci.yml::supply-chain::npm audit (client production dependencies) — BLOCKING`
- Catches a high or critical advisory in the dependency tree that actually
  reaches a browser. Clean at the commit this was introduced, so it needs no
  allowlist — which is the point: an allowlist of the fifteen devDependency highs
  the full audit reports would need maintaining and would fail every PR the day a
  new advisory lands in vite.

### npm-audit-report
- Command: `npm audit --audit-level=high` in `client/` and in `zk/`
- Workflow: `ci.yml::supply-chain::npm audit (zk, full)` and
  `npm audit (client, full)`, both `continue-on-error: true`
- **Reports, not gates,** and the reason is measured rather than assumed:
  `zk/`'s production tree carries 5 highs, all the
  `snarkjs → bfj → jsonpath → underscore` chain, and a build-time scan of
  `client/dist/assets/*.js` shows none of those names — nor `elliptic`,
  `ethersproject` or `secp256k1` from the `circomlibjs → ethers` chain — in the
  production bundle (CLAUDE.md, "snarkjs vulnerability containment"). `zk/` is
  build tooling, marked `"private": true`. Revisit when snarkjs drops bfj.

### codeql
- Command: n/a (`github/codeql-action`)
- Workflow: `.github/workflows/codeql.yml`, matrix over
  `javascript-typescript` and `actions`, query set `security-and-quality`,
  plus a weekly schedule because advisories land without a push.
- **Rust is deliberately not scanned.** It would need `security-extended` and a
  full build, which on Linux means the GTK / WebKit / soup / pipewire dev
  packages and a Tauri bundle — a second, slower copy of `check-desktop-linux`
  for a marginal signal. The substitute is `cargo clippy --all-targets -D
  warnings` plus `cargo deny`, and that trade is stated in the workflow rather
  than left implied.

---

## Release-pipeline gates

Everything below is in `.github/workflows/release-desktop.yml`.

### rel-preflight
- Workflow: `release-desktop.yml::preflight`
- Runs `version-sync.test.sh` and `verify-production-rejects-dev-fixtures.sh`
  BEFORE the builds, and `needs`-gates all three build jobs on it. Previously
  neither ran in any release path, so a mismatched tag or a dev-fixture proving
  key was discoverable only after 30-60 minutes of Tauri builds — or not at all.
- Both run again in `release`, unconditionally. A `workflow_dispatch` dry run
  reaches that job, and the workflow's own history includes a dry-run path that
  enforced LESS than the tag path.

### rel-server-tarball
- Workflow: `release-desktop.yml::build-server`
- The server had **no release artifact at all**: the only pipeline in this
  repository built the desktop app, and an operator who wanted to run a server
  had `docker build` from source.
- The tarball carries the binary, the ceremony-installed vkeys and the pinned
  VRP alignment model, because under the default posture a missing
  `membership_v2_vkey.json` is a hard startup error and under a production
  profile a missing alignment model is too — and both directories are gitignored.
  Migrations need no packaging; they are `include_str!`-ed into the binary.
- **The gate is `./annex-server --check`**, which runs the whole of
  `prepare_server` and exits. A tarball that cannot boot is not a release
  artifact, and "it built" does not answer that.

### rel-assets
- Workflow: `release-desktop.yml::release::Create GitHub Release`
- The `files:` list must include the updater BUNDLES, not only their `.sig`
  files. It did not until 2026-09-15: `latest.json` advertised
  `*.AppImage.tar.gz`, `*.nsis.zip`, `*.msi.zip` and `*.app.tar.gz`, none of
  which were uploaded, so every update 404'd and every signature check above it
  was inert. `SHA256SUMS` covers them and `latest.json` too — a user could
  previously checksum the installer they clicked and not the payload their
  machine fetches unattended.

### rel-notes
- Command: `python3 scripts/changelog-section.py CHANGELOG.md <version>`
- Workflow: `release-desktop.yml::release::Release notes from the CHANGELOG`
- The body was `generate_release_notes: true` alone — a list of commit subjects —
  while the hand-written `## [x.y.z]` section went unused. Fails the release when
  the section is absent.

---

## Federation gates

### signal-relay-contract
- Command: `node --test api/signal.test.mjs`
- Workflow: `.github/workflows/ci.yml::check-server::Signaling relay tests`, and `scripts/test-all.sh`.
- 59 tests over `api/signal.js`, the relay `crates/annex-federation/src/transport.rs` talks to — including the canonical signing string both sides must agree on byte for byte. Named by no workflow, script or doc until now, which is how the two implementations came to disagree about whether `rendezvous_tag` is part of that string.

---

## Documentation gates

### roadmap-consistent
- Command: `bash scripts/tests/roadmap-consistency.test.sh`
- Workflow: `.github/workflows/ci.yml::check-server::Harness script tests` (globbed).
- `ROADMAP.md` carries each phase's status in two places and they disagreed in five of them for months — the summary said PARTIAL, the phase section said COMPLETE. Also requires a PARTIAL phase to name the gap that keeps it partial, so the cheapest way to pass is not to delete the information.

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
- Workflow: `ui-audit` job, step `Functional browser suite`, against its own fresh server. Until that step existed this suite ran in NO workflow — it is the `chromium` Playwright project, and the audit job runs `--project=audit`, so defining both in one config had hidden the fact that only one of them was executed.
- Failure artifacts: screenshots in `client/e2e-results/`, HTML report in `client/e2e-report/`.

### e2e-group-call
- Command: `bash scripts/e2e-all.sh group-call`
- Workflow: `ui-audit` job, step `Group call lane`, against its own fresh server.
- Three real browser contexts with fake media devices join one channel serially, so each join exercises renegotiation against a room that already has peers in it. Asserts every participant sees itself plus two DISTINCT others — the property the SFU rearchitecture (`16a76b7`) exists to provide, and which the single-track fan-out could not.
- This is the guard that REPLACED the pinning test deleted when that landing happened. It was named by no script, no workflow and no doc until this entry, so between the rearchitecture and now, nothing ran it.

### e2e-puppeteer
- Command: `bash scripts/e2e-all.sh puppeteer`
- Workflow: `ui-audit` job, step `Puppeteer journey`, against its own fresh server.
- A second driver over the same journey — cold start, identity, in-browser proof, chat, channel create. It asserts: `fail()` prints and exits 1, and `main()` ends `.catch((err) => fail(...))`. Needs no browser of its own; `resolveChrome()` finds the Playwright-installed one.
- Run it with its OWN server, not shared: channel creation needs a moderator and `ensure_founder` grants that to the earliest registrant, so a shared server leaves this lane an ordinary member and it silently skips that check. `e2e-all.sh both` restarts between lanes for this reason.

### smoke-federation
- Command: `bash scripts/smoke-federation.sh`
- Workflow: `smoke-server-linux` job, step `Run federation smoke`.
- Boots a server and plays a remote peer whose Ed25519 key we control: seeds the post-handshake state, signs a real `FederatedMessageEnvelope`, POSTs it, and asserts it is accepted, persisted under the attested local pseudonym, idempotent on re-POST, and REJECTED when the signature is tampered with.
- Requires the `sqlite3` CLI. It reads the stored row directly and decrypts it with the server's at-rest key (HKDF-SHA256 over `signing.key` beside the database) — a plain string compare against `messages.content` fails, because non-E2E bodies are stored as `\x01ar1:base64(...)`.
- This is the only end-to-end evidence that two servers federate, and it was referenced by no workflow, script or doc before this entry.

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
- Updating: `bash scripts/ui-audit.sh --update-baselines`, committed separately and reviewed as a diff of images. Never update baselines in the same commit as the change that moved them without saying so. Delete the files first — `--update-baselines` only rewrites a baseline whose comparison FAILS, so a change landing just inside the 0.5% tolerance leaves the old PNG in place.

### ui-audit-order-independence
- Command: same run; two static checks in `client/e2e/audit/manifest.spec.ts`.
- Catches a surface that posts a message while `SEED.defaultChannel` is selected, and a surface clipped to `.chat-area` that does not mask `.message-view`.
- Why it exists: surfaces run serially against one server and one database, so a surface that WRITES to the channel other surfaces PHOTOGRAPH makes every later picture a function of run order. With `retries: 1` on CI, one genuine failure re-ran its `navigate`, posted a second copy of its message, and took 52 further surfaces down with it — 53 failures and 106 ledger findings from one defect, none of which said anything about the cause. Writes go to `SEED.channels.scratch` now, and these two checks are what stop that drifting back.
- Note: baselines are recorded on Linux/Chromium. Font hinting differs enough across platforms that re-recording on macOS or Windows will produce spurious diffs — record on Linux.

### ui-audit-a11y
- Command: same run; axe-core (WCAG 2.1 A/AA + best-practice) per surface per viewport.
- Catches: missing accessible names, contrast failures, heading-order breaks, duplicate landmarks, and — via a separate check — dialogs that do not move focus in, do not trap it, or do not close on Escape.
- Findings are recorded to `docs/ui-audit/findings.json` rather than asserted, so the run completes and reports everything; the ledger is reviewed as part of the PR.

### desktop-audit-package
- Command: `bash scripts/desktop-audit.sh`
- Workflow: `.github/workflows/ci.yml::desktop-audit`
- Catches what "does it build" cannot: a `.deb` that installs but leaves no binary on PATH; a `.desktop` entry that drops `x-scheme-handler/annex`, so every invite link a user clicks silently goes nowhere; a bundle that crashes during startup rather than at compile time; and an uninstall that leaves the binary behind. This is journey stage 01 — the first thing a real user touches — and none of it is reachable from the browser lane.
- Also runs `cargo test -p annex-desktop`, which `check-desktop-linux` skips. The script gates it on ~8 GB of free disk and reports a skip rather than dying mid-link, because the test binary links every Tauri Linux dep a second time.
- Layer 3 needs root or passwordless sudo for dpkg and is skipped cleanly without either; `--no-package` skips it explicitly.
- Failure artifacts: `/tmp/annex-desktop-launch.log`, uploaded by CI on failure.

### smoke-desktop-build
- Linux command: `bash scripts/smoke-desktop-build.sh`
- Windows command: `pwsh scripts/smoke-desktop-build.ps1`
- Workflow: not currently a separate CI job — `lin-build` / `win-build` plus the Tauri bundle gates in `release-desktop.yml` are a strict superset. Use this script locally as a fast pass/fail before pushing.
- What it covers: verifies the three release-critical ZK artifacts are present and that `membership_vkey.json` parses as JSON; runs `node scripts/build-desktop.js` (the same entry point Tauri's `beforeBuildCommand` uses, so the smoke stays on the bundle path rather than beside it); runs `cargo build -p annex-desktop --release`; confirms `client/dist/`, `client/public/zk/`, `zk/keys/membership_vkey.json`, and `target/release/annex-desktop[.exe]` exist as non-empty files.
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
