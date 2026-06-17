# Release Readiness & Packaging Proof

This document records how Annex is verified to be production-grade and how to
reproduce each proof. It is a runbook, not a status claim — every item below is
backed by a command you can re-run.

## Automated test surface (all green)

| Suite | Count | How to run |
|-------|-------|------------|
| Rust workspace (excl. annex-desktop) | 770 tests / 85 binaries, 0 clippy warnings | `cargo test --workspace --exclude annex-desktop` |
| Frontend (vitest) | 171 tests + eslint | `cd client && npm test && npm run lint` |
| ZK artifact gate | dev-fixture rejection under production profile | `cd zk && npm test` |
| Marketing-site invite router (`monolith-annex`) | 62 tests | `cd ../monolith-annex && npm test` |
| Server smoke (register → Merkle → Groth16 → verify → channel) | Linux + Windows | `bash scripts/smoke-server.sh` / `scripts/smoke-server.ps1` |

CI (`.github/workflows/ci.yml`, `workflow_dispatch` with `include_macos=true`)
runs the server checks, the **Linux + Windows + macOS** desktop builds, the
frontend tests, and the server smoke on **Linux + Windows** — all required jobs
green.

## Desktop packaging (Tauri 2)

### What builds, where

`cargo tauri build` produces, per platform:

- **Linux:** `.deb` + `.AppImage` (`bundle/deb`, `bundle/appimage`)
- **Windows:** NSIS `.exe` + `.msi` (`bundle/nsis`, `bundle/msi`)
- **macOS:** `.dmg` + `.app` (`bundle/dmg`)

The `.deb` bundles the binary at `usr/bin/annex-desktop`, the ZK verification
key, the Piper TTS binary + voice model, icons, and a `usr/share/applications/
Annex.desktop` entry carrying `MimeType=x-scheme-handler/annex` — i.e. the
`annex://` invite deep-link is registered with the OS at install time. The NSIS
installer registers the same scheme and ships `nsis/hooks.nsi`, which on
uninstall offers to remove `%APPDATA%\Annex`, the WebView2 data
(`%LOCALAPPDATA%\com.annex.desktop`), and logs.

### Verified install → run → uninstall cycle (Linux)

```bash
cd crates/annex-desktop
SKIP_PIPER=1 ANNEX_BUILD_PROFILE=dev cargo tauri build --bundles deb   # build (Piper staged via scripts/setup-piper.sh)
sudo dpkg -i ../../target/release/bundle/deb/Annex_*.deb               # install — binary on PATH, annex:// handler registered
xvfb-run -a annex-desktop                                             # launch — WebView loads the React frontend
sudo dpkg -r annex                                                    # uninstall — binary, .desktop handler, /usr/lib/Annex all removed
```

This full cycle has been exercised end to end. A Windows GUI installer cannot be
*executed* from a Linux build host, but the NSIS `.exe`/`.msi` are built by CI on
a real Windows runner and the install/uninstall hooks are reviewed above.

### Downloadable installer artifacts

`ci.yml` proves the installers **build** on every platform but does not upload
them. To get installable artifacts you can download (and actually run on
Windows/macOS), use **`.github/workflows/package-proof.yml`** (Actions →
"Package Proof" → Run workflow). It builds the full bundles on all three
platforms under `ANNEX_BUILD_PROFILE=dev` with freshly generated dev-fixture ZK
keys and uploads them. Packaging correctness is independent of key provenance;
the dev-fixture keys are clearly **not** a production release.

> `package-proof.yml` only becomes dispatchable once it exists on the default
> branch (a GitHub `workflow_dispatch` constraint).

### Release pipeline caveat (must fix before a real public release)

`release-desktop.yml` builds under `ANNEX_BUILD_PROFILE=production`, which makes
`zk/scripts/verify-artifacts.js` enforce the pinned manifest. The pinned
artifacts (`zk/build/membership.r1cs`, `zk/keys/membership_final.zkey`, etc.)
are **gitignored and not generated in the workflow**, and `dev-setup-groth16.js`
uses random entropy, so the verify step fails with `MISSING-FILE` before the
bundler runs — on every platform. Before tagging a real release the project must
either (a) run a real multi-party trusted-setup ceremony and commit/host the
resulting artifacts so the manifest hashes resolve, or (b) make the dev-fixture
deterministic and add a generation step. Until then, `release-desktop.yml`
cannot produce a build; use `package-proof.yml` for installable (non-production)
artifacts.

## Invite link router (through the marketing site)

End-to-end path, verified by tests in both repos plus a cross-repo contract
check:

1. `annex-server` `POST /api/invites` → `https://monolithannex.com/invite/<base64url(JSON{server,code,...})>`
2. `monolith-annex` (Vercel) decodes the payload, validates the server URL
   (HTTPS-only, rejects private/reserved IPs), renders an OG social preview and
   an **"Open in Annex"** button pointing at `annex://invite?server=…&code=…`
3. `annex-desktop` `deep_links.rs` parses that `annex://` URL (HTTPS-only) and
   hands `{server, code}` to the frontend, which redeems it.

The three independent implementations of the format (Rust encoder, JS
decoder/emitter, Rust deep-link parser) agree on vanilla payloads, special
characters in the code, and HTTP/private-IP rejection.

## E2E (Playwright + Puppeteer)

```bash
bash scripts/e2e-server.sh start            # builds client + server, fresh DB, :3000
cd client && npm run test:e2e               # Playwright: 12/12
node e2e-puppeteer/run.mjs                   # core flow + cold-start, screenshots
node e2e-puppeteer/voice.mjs                 # single-party WebRTC voice/video
node e2e-puppeteer/voice-video.mjs           # two-party VIDEO fan-out (getStats proves inbound VP8)
bash scripts/e2e-server.sh stop
```

Playwright screenshots land in `client/e2e-results/`; Puppeteer screenshots in
`client/e2e-puppeteer/screenshots*/`. The Playwright `completeStartup` helper
drives the app's real recovery path (clicking **Retry** if a transient "Unable
to contact server" appears) so the suite is reliable under load rather than
flaking on a recoverable error screen.
