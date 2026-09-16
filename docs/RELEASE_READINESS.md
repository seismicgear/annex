# Release Readiness & Packaging Proof

This document records how Annex is verified to be production-grade and how to
reproduce each proof. It is a runbook, not a status claim — every item below is
backed by a command you can re-run.

## Automated test surface (all green)

| Suite | Count | How to run |
|-------|-------|------------|
| Rust workspace (excl. annex-desktop) | 1284 tests / 134 binaries, 0 clippy warnings | `cargo test --workspace --exclude annex-desktop` |
| Frontend (vitest) | 502 tests across 54 files + eslint + `tsc -b` (which now covers the test files themselves — see below) | `cd client && npm test && npm run lint && npx tsc -b` |
| Playwright functional suite | 13 tests | `bash scripts/e2e-server.sh start && cd client && npm run test:e2e` |
| Group call (3 real browser contexts, fake media) | 6 tests | `bash scripts/e2e-all.sh group-call` |
| Harness scripts | 12 files | `for t in scripts/tests/*.test.sh; do bash "$t"; done` |
| Federation signaling relay | 59 tests | `node --test api/signal.test.mjs` |
| ZK proof round-trip (57 assertions: tamper rejection, and the v2 challenge binding) | `zk-proof` gate | `cd zk && node scripts/test-proofs.js` |
| Production ZK gate (refuses what it should, is wired into the release, AND is invoked by CI — it was not) | 14 assertions | `sh scripts/verify-production-rejects-dev-fixtures.sh` |
| Live federation relay (signed envelope, second server) | 1 end-to-end path | `bash scripts/smoke-federation.sh` |
| Puppeteer journey (cold start → identity → proof → chat → channel create) | 1 driver-independent pass | `bash scripts/e2e-all.sh puppeteer` |
| UI audit (screenshots + a11y + console + network + overflow + keyboard) | 105 surfaces, 413 captures + 9 manifest checks + 3 setups = 425, 0 findings | `bash scripts/ui-audit.sh` |
| Desktop install → run → uninstall | 10 checks, and the only lane that RUNS the binary | `bash scripts/desktop-audit.sh` |
| ZK artifact gate | dev-fixture rejection under production profile | `cd zk && npm test` |
| Marketing-site invite router (`monolith-annex`) — **cross-repo, not verifiable from this checkout** | 62 tests as last reported | `cd ../monolith-annex && npm test` |
| Server smoke (register → Merkle → Groth16 → verify → channel) | Linux + Windows | `bash scripts/smoke-server.sh` / `scripts/smoke-server.ps1` |

The counts above are what the commands beside them printed, not a target. If a
number here disagrees with a run, the run is right and this table is stale —
that has already happened twice. It once claimed 770 Rust and 171 frontend
tests against actuals of 1055 and 469; and it carried "16 assertions" for the
ZK proof round-trip through four commits in which that script did not run at
all, because `membership_v2` gained a `challenge` public input and the script
built its witness without one. A count in a table is not a run.

Two additions worth naming rather than burying in a number:

* **The frontend test files are now typechecked.** Vitest transpiles with
  esbuild and cannot fail on a type error, so until `client/tsconfig.test.json`
  existed, `src/**/*.test.ts(x)` and `e2e/` were outside every `tsc -b` in the
  repo. A test file with a type error passed by not being checked.
* **The production ZK gate is invoked.** It asserted that a release runs
  `verify-artifacts.js --all`, and nothing ran the gate itself: CI's globbed
  step matches `scripts/tests/*.test.sh` and the script is `scripts/*.sh`. It
  is an explicit step in `ci.yml::check-server` and in `scripts/test-all.sh`
  now, and it asserts both callers exist.

CI (`.github/workflows/ci.yml`, `workflow_dispatch` with `include_macos=true`)
defines the server checks, the **Linux + Windows + macOS** desktop builds, the
frontend tests, the UI audit lane, and the server smoke on **Linux + Windows**.

Rows here have a way of being defined and not run. Five were, until recently:
the Playwright functional suite, the group-call lane and the puppeteer journey
were named by no workflow, `scripts/smoke-federation.sh` was referenced by no
workflow, script or doc, and the harness scripts had no tests.

Two more were found the same way and are wired now. `api/signal.test.mjs` is 41
tests over the federation signaling relay — including the canonical signing
string `crates/annex-federation/src/signal.rs` has to match byte for byte — and
was named by nothing, which is how the two implementations came to disagree
about whether `rendezvous_tag` is part of that string. And `zk-proof`, a gate
`docs/refactor/release-gates.md` describes in detail, ran in no workflow: CI's
`zk npm test` covers `verify-artifacts.js` only, not the script that generates
and verifies real proofs.

Defining a suite is not running it — every row here now names a command AND a
job, and `scripts/test-all.sh` runs the ones that need no browser.

### The sweep behind these numbers

Every row above was run on one machine, in sequence, at `f491be8` (2026-09-16) —
not assembled from different commits on different days:

| Lane | Result |
|---|---|
| `bash scripts/test-all.sh` | all steps passed, exit 0 |
| `cargo test --workspace --exclude annex-desktop` | 1284 passed, 0 failed, 134 binaries |
| `cd client && npm test` | 502 passed, 54 files |
| `for t in scripts/tests/*.test.sh` | 12 files, 0 failed |
| `node --test api/signal.test.mjs` | passed |
| `cd zk && node scripts/test-proofs.js` | passed |
| `sh scripts/verify-production-rejects-dev-fixtures.sh` | passed |
| `npm run test:e2e` | 13 passed |
| `bash scripts/e2e-all.sh group-call` | 6 passed |
| `bash scripts/e2e-all.sh puppeteer` | passed, message round-trip confirmed |
| `bash scripts/smoke-server.sh` | OK |
| `bash scripts/smoke-federation.sh` | OK — signed accept, persist, idempotent re-POST, tamper reject 401 |
| `cargo check -p annex-desktop` + `clippy --all-targets -D warnings` | clean |
| `cargo test -p annex-desktop` | 28 passed |
| `bash scripts/desktop-audit.sh` | 10 passed, 0 failed |
| `bash scripts/ui-audit.sh` (record, then an independent verify) | 425 passed, 105 of 105 surfaces, 0 findings, **both runs** |

The desktop audit is the row to read twice. On the commit before it ran,
`cargo check`, `cargo clippy --all-targets`, `cargo test -p annex-desktop` and
`cargo tauri build` were all green while the installed binary exited before its
first window — an updater plugin registered against a config section that a
build without the signing secret does not have. None of those four lanes runs
the binary. The Xvfb launch step is the only thing in this repository that
could have caught it, and a release built from that commit would have shipped
an app that does not start.

**The sha is `f491be8` and not `a96e275` for a reason worth keeping.** This block
first named the earlier commit, and at that commit `cargo fmt --all --check`
would have failed — a stray blank line from a scripted edit, made after the
suite last ran. The table was therefore describing a state that never existed at
once, which is the failure it exists to prevent. The fix was to format, commit,
and run the suite again at the result, not to edit the sha.

**What this sweep is not.** It is a local run on Linux. It says nothing about
Windows or macOS, and CI has not executed on this branch at all — see below.

> **CI executes when it gets runners, and whether it gets them is not
> reliable.** The paragraph that first stood here said CI proved nothing, because
> every job finished in three to four seconds with `runner_id: 0` and no steps.
> Its replacement said that was over. Both were overstated in the same way:
> allocation is intermittent, and each paragraph read one sample as a standing
> condition.
>
> What is measured. Runs 741–748 all died in three to six seconds with no
> runner. Run `34001650423` (749) failed that way on attempt 1 and ran 78
> minutes on attempt 2. Run `34006312223` (750) on `main` (`b172edf`) occupied
> real runners for 41 minutes: `Check (Server)`, `Frontend Tests`, both desktop
> builds, both server smokes, the federation smoke and the desktop audit all
> passed; `UI Audit (Linux)` failed; macOS was skipped by design. Run
> runs 751 through 755, dispatched on
> `claude/beautiful-bohr-9qb34p` over about ninety minutes, ALL died in three to
> ten seconds — six attempts counting 751's re-run, five jobs each time,
> `ubuntu-latest` and `windows-latest` alike, `runner_id: 0`, `runner_name`
> empty, and HTTP 404 for the job logs because no log was ever written. The five
> jobs that `needs` them were skipped, so each run reports as a failure with
> nine of ten jobs having executed nothing.
>
> **So this branch has no CI result, and nothing here should be read as one.**
> The lanes were run locally instead — see "The sweep behind these numbers"
> above — which covers Linux and says nothing about Windows or macOS.
>
> **So tell the two apart before reading either one as a result**, in this
> order, because they render identically in the checks list:
>
> 1. `runner_id` is `0` and `runner_name` is empty → nothing ran. A job that ran
>    names its runner.
> 2. The job's log download 404s → nothing ran. A job that ran has a log, even
>    if it failed in its first step.
> 3. Elapsed time is a few seconds on a job whose fastest step is a `cargo`
>    fetch → nothing ran.
>
> None of that excuses a red check that has a runner, a log and a duration.
> Re-running the workflow is the response to the first kind; it worked for 749
> and did not for 751.
>
> Leaving that paragraph in place was the more dangerous of the two errors it
> could make. It instructed the reader to discount a red check as
> infrastructure — and a red check was sitting on `main` at the time, which is
> exactly what it told them to ignore.
>
> **A red check on this repository is a real failure.** Before theorising about
> the cause, download the `ui-audit-report` artifact from the run: it carries
> `client/e2e-results/<surface>-{actual,diff}.png`, the masked and clipped
> actual beside a pixel diff. Reading `diagnostics/` instead is what produced
> three wrong diagnoses of the same failure — that directory holds an
> unmasked, unclipped full-page shot, which cannot be compared to a baseline.

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

### The trusted setup, and what it does and does not prove

`release-desktop.yml` builds under `ANNEX_BUILD_PROFILE=production`, which makes
`zk/scripts/verify-artifacts.js --all` enforce the pinned manifests and
`zk/scripts/verify-ceremony.js` check that a ceremony produced them.

The ceremony is **single-operator with a public beacon**, and the manifests say
so: `ceremony.type` is `single-contributor-beacon`, never `mpc`. `ceremony.js`
runs the standard Groth16 construction — phase 1, then a phase-2 contribution
per circuit — and finalises with a drand round (League of Entropy, BLS-signed,
verifiable by anyone forever). The round number is **committed to before that
round exists**, together with the hashes of the pre-beacon artifacts, so nobody
— including whoever ran it — could know the beacon while choosing a
contribution, and nobody could steer the result.

What it does not provide is independent participants. A multi-party ceremony
whose contributors do not trust each other is strictly stronger, and it remains
the target. Two things make the upgrade cheap: `ceremony.js --contributors N`
already records each round in the transcript, and `--ptau <file>
--ptau-sha256 <hex>` adopts a real perpetual Powers of Tau (hash-checked, then
`powersoftau verify`-ed) in place of the locally generated phase 1.

Verify any of that from a clean checkout, offline except for the beacon check:

```bash
ANNEX_BUILD_PROFILE=production node zk/scripts/verify-artifacts.js --all
node zk/scripts/verify-ceremony.js
sh scripts/verify-production-rejects-dev-fixtures.sh
```

The third one tests the **gate** rather than the tree: it builds throwaway
manifests and asserts each is refused for the right reason and with the right
exit code. Its predecessor checked one manifest of five, treated any non-zero
exit as proof, and never neutralised `ANNEX_ALLOW_DEV_CEREMONY` — so it passed
while the release workflow gated a single circuit and nothing stopped the
bypass being set.

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
cd client && npm run test:e2e               # Playwright functional suite: 13/13
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
