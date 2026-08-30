# Annex — Claude Code Guide

## Project Overview

Tauri 2.x desktop app: Rust backend (Axum) + React/TypeScript frontend + ZK circuits.

- **Backend**: 12 Cargo workspace crates in `crates/`
- **Frontend**: React 19 + Vite + Zustand in `client/`
- **ZK**: Circom circuits + snarkjs in `zk/`
- **Desktop**: Tauri 2 shell in `crates/annex-desktop/`. Compiles cleanly when (a) the GTK / WebKit / libsoup / pipewire dev packages from `scripts/claude-setup.sh` are installed and (b) the gitignored `assets/piper/` and `assets/voices/` directories exist (the Tauri bundler validates bundle.resources at build time). The crate is excluded from default cargo checks for environments that don't have those system deps; once they are installed, `cargo check -p annex-desktop` succeeds. See "Desktop crate build" below.

## Environment Setup

Run the setup script to prepare the environment (also runs automatically on session start):

```bash
bash scripts/claude-setup.sh
```

This installs system dependencies (WebKitGTK, GTK, PipeWire), generates ZK keys if missing,
installs frontend npm deps, and verifies Rust compilation.

## Test Commands

### Run all tests (recommended)
```bash
bash scripts/test-all.sh           # fmt + clippy + eslint + cargo test + tsc + npm test
bash scripts/test-all.sh --quick   # skip fmt/clippy/eslint; still typechecks
bash scripts/test-all.sh --verbose # show test output (--nocapture)
```

### Individual test suites
```bash
# Rust tests. annex-desktop is excluded because it needs the GTK/WebKit/soup/
# pipewire dev packages, not because it is broken — see "Desktop crate build".
# With those installed it compiles and clippies clean.
cargo test --workspace --exclude annex-desktop

# Run a specific test file
cargo test -p annex-server --test api_channels_crud

# Run a specific test
cargo test -p annex-server --test api_channels_crud -- test_create_channel_success

# Frontend tests
cd client && npm test

# Single frontend test file
cd client && npx vitest run src/stores/channels.test.ts
```

### E2E visual tests (Playwright)
```bash
# Start the E2E server (builds client + starts Axum on port 3000)
bash scripts/e2e-server.sh start

# Run E2E tests
cd client && npm run test:e2e

# Stop the server when done
bash scripts/e2e-server.sh stop

# Run a specific E2E test file
cd client && npx playwright test e2e/startup.spec.ts

# Screenshots are saved to client/e2e-results/ on failure
# HTML report is generated at client/e2e-report/
```

### UI audit (screenshots + accessibility)
```bash
# Full run: fresh server, capture every surface at 4 viewports, audit each,
# diff against committed baselines, render a contact sheet.
bash scripts/ui-audit.sh

# Iterate on one surface
bash scripts/ui-audit.sh --grep admin-server-policy

# Re-record baselines after an intended visual change (separate commit)
bash scripts/ui-audit.sh --update-baselines
```

Surfaces are declared in `client/e2e/audit/surfaces.ts` — that file is the
coverage contract, and `manifest.spec.ts` fails if a component renders a modal
no surface reaches. Baselines live in `client/e2e/audit/baselines/` and are
tracked; the ledger and contact sheet land in `docs/ui-audit/`. Full docs:
`docs/ui-audit/README.md`.

#### Operating the audit

A run takes ~11 minutes and is easy to invalidate. Four rules, each learned by
invalidating one:

- **Never start a run while another is still going.** `scripts/ui-audit.sh`
  now takes an exclusive `flock` and refuses with a clear message rather than
  trusting anyone to remember this, because writing the rule down here did not
  stop it happening a second time. `e2e-server.sh start`
  stops whatever is on port 3000 before starting its own server, so a second
  run silently kills the first one's server mid-capture and the two then fight
  over the port. It does not look like a collision from either side: the
  victim reports ordinary capture failures, and the new run fails its founder
  setup with "founder must be the earliest registrant" because it is talking
  to a database that is not the one it created. Launching record+verify as one
  background command is not enough — wait for the VERIFY to finish, not just
  the record.
- **`--grep` cannot validate every surface.** The surfaces share one database
  and several post messages, so an unclipped capture — `error-boundary` and
  `agent-detail-overlay` are the two — shows a message column whose contents
  depend on which surfaces ran BEFORE it. Under `--grep` those never ran, so
  the picture legitimately differs from a baseline recorded in a full run and
  the surface fails for a reason that is not a defect. Use `--grep` to check
  that a surface is REACHABLE and that its assertions hold; use a full run to
  judge its pixels.
- **Never run `cargo` alongside a run.** The server is built at run start; a
  concurrent cargo command takes the build lock and starves it. Once
  `Server ready` appears in the log the lock is free, but CPU contention can
  still push a capture past its 45s budget. Both halves of that are real: a
  `cargo check` started by accident mid-run did not starve the build, it
  starved the CPU, and `channel-empty-state @ mobile` photographed
  "Loading channel history…" and failed by 19,190 pixels — 0.058 against a
  0.005 tolerance, which looks nothing like noise and is not a regression.
- **The audit lock now guards the PORT, not just a second audit.**
  `e2e-server.sh` consults `${TMPDIR:-/tmp}/annex-ui-audit.lock` before
  touching port 3000 and refuses while a run holds it; `ui-audit.sh` exports
  `ANNEX_AUDIT_CHILD=1` so its own server calls pass. This was added after
  `e2e-all.sh bogus` — a mistyped lane, which that script validated only
  AFTER starting a server — killed a run at surface 52 of 415. The argument
  is validated first now, and both fixes exist because either alone leaves
  the hole open.
- **`flock <file> -c '<cmd>'` does not release the lock when you kill
  flock.** The command runs as a child and inherits the locked descriptor, so
  a `flock lock -c 'sleep 25'` you kill keeps the lock for the rest of the
  25s — and the next `ui-audit.sh` refuses to start for no visible reason.
  Hold a lock from a process that can be killed directly (a `python3 -c`
  doing `fcntl.flock` then sleeping) instead.
- **Never edit `client/src` mid-run.** `e2e-server.sh start` builds the client
  once, at the beginning. Editing after that means the run no longer
  corresponds to the working tree, and its result means nothing.
- **Never commit while a `--update-baselines` run is in flight.** A partially
  re-recorded baseline set looks exactly like a complete one in `git status`.
  `64530f6` shipped four surfaces whose baselines were still being written,
  and CI could not heal it: `updateSnapshots: 'none'` refuses to mint a
  snapshot for any reason, so it failed until the files were committed from a
  machine that had recorded them.
- **A green run is only evidence if the tree is clean afterwards.** Zero
  baseline drift across 252 captures is what proves a mechanical change (a
  token migration, a rename, a refactor) changed no pixels. That claim is
  worth more than reading the diff.
- **`git status` is not a measure of visual change — count pixels.** PNG
  bytes are not reproducible run to run: re-recording a baseline that is
  visually identical still rewrites it, because font rasterisation moves a
  handful of anti-aliased pixels. Measured on this repo, an unchanged
  `.chat-area` capture comes back with 8-16 differing pixels out of 711,760
  — a ratio of 0.00002 against a 0.005 tolerance, 250x below the threshold.
  So a `git status` listing 54 modified baselines can mean *nothing* moved.
  A claim about what a change did to the pixels has to come from decoding
  the two PNGs and counting; one such claim in this session's history
  ("139 baselines were carrying mask rectangles") was about 2x overstated
  because it read `git status` instead. Sampling the same set properly put
  it at roughly half real, half noise — and two of the real ones came in at
  0.0022 and 0.0003, under the tolerance, which is exactly why the
  baselines have to be deleted before re-recording rather than left for
  `--update-baselines` to notice.
- **A recording run proves nothing.** `--update-baselines` rewrites whatever
  it sees, so it cannot fail on drift and cannot tell you the guard holds.
  Every claim about the audit comes from a plain run afterwards, against the
  baselines that were just recorded.
- **`--update-baselines` rewrites a baseline only when its comparison
  FAILS.** A change that lands just inside `maxDiffPixelRatio: 0.005` leaves
  the old PNG in place, so a surface you know you changed can come out of a
  recording run still carrying its previous baseline — and then fail a later
  run when ordinary anti-aliasing noise tips it over. It looks exactly like
  flakiness, and it is not. When you have changed something and want its
  baseline definitely regenerated, `rm` the file first (with an ABSOLUTE
  path — a relative one from `client/` silently deletes nothing); a missing
  snapshot is always written. The tolerance is looser than it sounds:
  rewriting a whole sentence of body text inside a 520x576 dialog came in
  UNDER 0.005 and left the old PNG in place, so the capture showed the new
  wording and the baseline kept the old. Text changes always need the delete. This cost two full cycles and a wrong diagnosis: the first
  reading blamed a username-resolution race, and the actual cause was a
  stale baseline the recording run had declined to rewrite.

- **A capture's scroll position can be a measurement rather than a decision.**
  Under 760px the channel list is a horizontal strip. Playwright's `click()`
  scrolls its target into view *only if it judges it not already visible*, and
  that judgement is a width comparison — against chips whose emoji icons have
  a font-dependent advance, the same swing that made three other surfaces
  flaky. `mobile/chat-main` sat still for five runs and then flipped to the
  unscrolled position: 12,422 differing pixels, 0.038 against a 0.005
  tolerance, nothing about it resembling noise. `scrollIntoView` is not the
  fix — it has the same "only if needed" clause and can silently no-op, which
  is how it was diagnosed as working when it was not. Set `scrollLeft` from
  two `getBoundingClientRect()` calls instead: arithmetic cannot decline.
- **A bubble that is still `pending` is 21px wide-of-the-mark, and that was
  read as a scrollbar.** `mobile/message-edit-refused` came back with its
  bubbles 21px left and a reply icon that had never been in frame; the first
  diagnosis here said a scrollbar gutter had appeared, hedged as unproven.
  It was wrong. A run-length scan across the row settles it: the right gutter
  is the container's own 16px padding in BOTH images, and the extra 21px is a
  button. `MessageView` withholds the reply control while
  `message.pending`, the action row is right-aligned beside the bubble, and
  losing a control moves the bubble. `postFreshMessage` asserted the send had
  not FAILED but never that it had been CONFIRMED, so every surface built on
  it photographed whichever side of the confirmation the network was on. It
  waits for `.pending-status` to clear now. The lesson is the older one
  restated: when a helper waits for something to appear, ask what the thing
  looks like in every state it can appear in.
- **The pointer's resting place is in the picture.** It stays where the last
  click left it, and hover affordances render. Moving a strip under a resting
  pointer re-hovers whatever slid beneath it, so `mobile/chat-main` came out
  with one channel selected and its neighbour showing hover controls. After
  any programmatic scroll, put the pointer back.
- **`pgrep -f "ui-audit.sh"` matches the watching shell itself**, so
  `until ! pgrep -f "ui-audit.sh"; do sleep …; done` never exits. Watch the
  log for `[ui-audit] done`.
- **A restarted session does not stop a run.** The shell and any watchers die
  with it; a `nohup`-ed `ui-audit.sh` keeps going, and the first evidence is
  the lock still being held. Check `pgrep -af ui-audit.sh` and the lock before
  concluding a run is gone — starting a second one on top is the collision
  everything above exists to prevent. The same restart is why a baseline can
  come back modified with no run of yours in sight, and why reverting one
  mid-run leaves that single file out of step with the rest of the set: the
  verify pass afterwards is the arbiter, not `git status`.
- **`playwright test` runs every project**, so `testIgnore` on one project
  does not keep another out of a bare invocation. `npm run test:e2e` was
  pulling in the sixteen-minute audit lane — against a database the functional
  suite had already posted into — from a command documented as the E2E suite.
  The scripts name `--project=chromium` explicitly now.

#### Writing a surface

The surfaces share one server and one database, in manifest order, and three
of the four ways to break that were learned by breaking it.

- **A surface that writes to shared server state contaminates the ones after
  it.** `message-image-lightbox` first stubbed the upload endpoint to return a
  made-up URL. The message was real and went into the shared channel; the URL
  resolved to nothing, so every later surface that opened that channel logged
  a 404 — 58 findings from one surface, none of them about the app. Either
  leave no trace, or leave one that works: it now uploads for real.
- **A surface that reads shared server state is order-dependent across
  viewports.** `channel-encryption-enabled` provisioned a real channel key at
  the desktop viewport and then failed at the other three: each context has a
  fresh device key, so the channel came back keyed-with-nothing-for-us — the
  pending state, captured under the name of the ready one. It stubs both key
  routes now, so every viewport reaches the same state.
- **Clip narrowly.** `.chat-area` drags in the auto-scrolling message column,
  and any surface doing async work after the history settles can photograph it
  mid-scroll. The audits run against the whole page regardless of the clip, so
  narrowing the picture costs no coverage.
- **A deliberate failure needs a waiver on `network` AND `console`.** The
  browser logs an injected 500 to both.

#### Defect classes this codebase actually produces

Every serious defect found by the audit and the two defect sweeps fell into one
of these. Look for them first:

1. **A failure rendered as an ordinary result.** A dropped request becomes "no
   results", an empty list, or a saved edit. The user is told something true-
   looking that the server never said. Found and fixed 8×.
2. **A value that never crosses a boundary it is assumed to cross.** A field
   added to a response struct but dropped by a hand-built `json!` at the edge;
   a resolved config value written into `AppState` and nowhere else. Every
   layer is individually correct and the feature is inert. Found 3×.
3. **A check keyed on a different identifier than the query it guards.** The
   route takes two ids, authorization reads one, the data reads the other.
4. **A default that contradicts another default.** `voice_enabled: true` with a
   loopback WebRTC URL; `agent_min_alignment_score: 0.8` with no principles to
   score against; a 3-hour retry budget against a 5-minute freshness window.
5. **A non-happy-path branch that drops structure the happy path provides** —
   an error state rendered without the landmark, heading, or list the normal
   state has. Found 4×.
6. **A generic failure branch standing beside specific siblings.** One
   `catch` collapses several distinct causes into one message, or discards a
   message the layer below already produced. It reads as merely terse and is
   usually worse than that: `beginRemoteRegistration` returned `null` for
   both an unreachable server and a failed local identity clone, so five
   callers told the user to check a network that was fine.
   `normalizeServerUrl` threw "Only http and https URLs are supported." and
   the caller replaced it with "Invalid URL format." Look for a branch whose
   neighbours name the thing that failed while it does not. Found 3×.
7. **A read-then-write transaction opened DEFERRED.** Under WAL the read takes
   a snapshot and the write has to upgrade; a concurrent commit turns that into
   `SQLITE_BUSY_SNAPSHOT` *immediately*, with the busy handler never invoked,
   so `busy_timeout` cannot help. It surfaces as an intermittent
   "database is locked" on a perfectly ordinary operation. Every write
   transaction in this repo is `BEGIN IMMEDIATE`. Found in
   `edit_message`/`delete_message` first, then in `send_message` a release
   later, then in eleven more sites — the rule was written down here after
   each one and re-broken anyway, because the only thing enforcing it was
   whether the next person had read this paragraph. It is checked now:
   `annex-server/tests/write_transactions_are_immediate.rs` scans the
   workspace sources and fails on a plain `.transaction()`, an
   `unchecked_transaction()`, or a `transaction_with_behavior` that is not
   `Immediate`, naming the file and line. Source-scanning rather than
   behavioural on purpose — the defect is a race, so a behavioural test for it
   is either slow and flaky or passes for the wrong reason;
   `tests/ws_send_immediate_tx.rs` covers one site that way. A `savepoint()`
   is DEFERRED underneath and is deliberately NOT flagged: the one in
   `annex-federation::create_agreement` writes before it reads, so it takes
   the write lock at its first statement and the busy handler still applies.

The corollary: unit tests do not catch (1)–(4), because in every case the unit
under test was correct. They are only visible at the boundary — an HTTP-level
test, or the browser.

And a corollary to that: **the harness can have defect (1) too.** The audit's
`postFreshMessage` waited for the message bubble, which is optimistic — it
appears when the frame leaves the device and stays there, marked `failed`, if
the server refuses. So a failed send passed the helper, the surface
photographed a failed bubble and a composer error, and `--update-baselines`
wrote it down as the correct appearance. A real intermittent server bug (6,
above) lived inside a committed screenshot. When a helper waits for something
to *appear*, ask what it looks like when the operation failed; if the answer is
"the same, plus a badge", the helper is asserting nothing.

The same question applies to the harness's own process handling.
`e2e-server.sh` reported `Killing stray process on port 3000 (PID …)` and
killed nothing, then reported `Server ready on port 3000 (PID …)` two seconds
later — naming a process that was on its way to exiting with `AddrInUse`,
while the survivor answered the health check. The run then drove a database
the script had not created, and the symptom arrived much later as
**"founder must be the earliest registrant"** — the failure CLAUDE.md already
described as looking like an ordinary capture problem from both sides. Two
causes, each silent:

- **`lsof -ti :PORT` matches CLIENT sockets, not just the listener.** Any
  browser context with an open connection is a second pid, so the value is
  multi-line — and `kill` rejects such a string whole ("arguments must be
  process or job IDs") without signalling anyone. Verified: with one
  connection open the list was `<server> <client>` and the kill was a no-op.
  Use `lsof -ti tcp:$PORT -sTCP:LISTEN`, and signal each pid separately.
- **`curl /health` cannot tell your server from someone else's.** Liveness of
  the pid you launched is not enough either: a server still starting up is
  alive while the stranger answers. `cargo run` uses `exec_replace` on Unix,
  so `$!` *is* the server — compare it against the listening pid.

Pinned by `scripts/tests/e2e-server-port.test.sh`, which is in
`scripts/test-all.sh` and in the `Check (Server)` CI job. Two things that
test taught, both of which made it lie before they were fixed: a killed
background child is a **zombie** until the shell reaps it and `kill -0`
succeeds on a zombie (read the state field of `/proc/<pid>/stat` instead),
and **sourcing a script imports its `set -euo pipefail`** — an `lsof` that
matches nothing then exits 1, aborts the EXIT trap under `pipefail`, and
takes the script's exit status with it, so it reported "0 failed" and
exited 1.

#### The report is not the run

`docs/ui-audit/index.html` said "103 surfaces captured · 0 findings" —  the
phrase this project reads as proof of health — after a run that captured
nothing at all. `loadShots()` counted the TRACKED baselines directory, which
no run clears, so the previous run's pictures were presented as this one's
evidence, and the same run overwrote the tracked `findings.json` with an empty
list. Reproduced by deleting the ledger and running the generator alone.

`capture.spec.ts` now appends one line per surface it STARTS to
`docs/ui-audit/captured.jsonl` (gitignored, cleared at run start), and the
report says "N of M surfaces exercised in this run", banners a partial or
empty run, labels the cards it did not reach, and refuses to rewrite the
tracked `findings.json` unless the run was full. Recorded at start rather
than after a successful screenshot on purpose: the run whose surfaces mostly
FAILED is the one whose findings matter most, and it must not be mistaken for
a partial sweep. Pinned by `scripts/tests/ui-audit-report.test.sh`.

#### Lanes that exist but run nowhere

`playwright.config.ts` defines four projects. `ui-audit.sh` runs
`--project=audit`; `npm run test:e2e` runs `--project=chromium`. Until this
was checked, **neither of the other two ran in CI at all**:

- the functional suite (`client/e2e/*.spec.ts`) — identity creation, the
  in-browser Groth16 proof, channel join, send, edit, delete, reply, admin
  channel CRUD, A-to-B delivery over the WebSocket;
- `group-call`, which is the guard that REPLACED the pinning test deleted
  when the SFU rearchitecture landed, and was named by no script, no
  workflow and no doc.

The puppeteer harness (`client/e2e-puppeteer/`) was the third. It was very
nearly dismissed here as "a screenshot tour with no assertions to fail" —
reading it disproved that in one grep: `fail()` prints and calls
`process.exit(1)`, it is used a dozen times, and `main()` ends
`.catch((err) => fail(...))`. It drives the same journey through a different
browser driver, including a cold start, and it needs no browser of its own
because `resolveChrome()` finds the Playwright-installed one.

All three are steps in the `ui-audit` CI job now, each against its own fresh
server, and `group-call` has an entry point in `e2e-all.sh`. Two lessons: a
project in a Playwright config is not a lane until something names it, and a
suite is not decorative because you assumed it was — check before writing the
assumption into a comment that outlives you.

#### Tests that stop testing what they claim

A third corollary, and the one that hides longest: **a test can pass for the
wrong reason from its second run onward.**

`config::tests::defaults_are_loaded_when_file_missing` passed the relative
literal `"this-file-does-not-exist.toml"` to `load_config`, which — by design,
as the first-run bootstrap — *created* it in the crate root, complete with a
freshly derived `server_slug`. From then on the file existed, `load_config`
took the parse branch, and the missing-file branch the test exists to cover was
never executed again on that machine. It still passed, because the file it was
now reading happened to hold defaults. Append `[logging] level = "trace"` to
the leftover and the test fails: proof that its result depended on an untracked
file in the source tree. The leftover had been gitignored with a note claiming
"the path name is load-bearing for the test", which it was not — only its
absence was.

The tell is a test whose fixture is a *path* rather than a *file*, and any test
that asserts on the absence of something the code under test may create. Give
it a unique path, assert the absence before the call, and clean up after.

#### Sourcing a script to test it imports its variables too

`scripts/tests/*.test.sh` drive shell helpers by `source`-ing the script under
test (each one stops at a `BASH_SOURCE` guard before its body). Two things
come across with the functions and both have bitten:

- **Its shell options.** `e2e-server.sh` sets `set -euo pipefail`, so the test
  inherited errexit; an `lsof` that matched nothing then exited 1, aborted the
  EXIT trap under `pipefail`, and took the script's exit status with it — it
  printed "0 failed" and exited 1. Re-set the options you want after sourcing.
- **Its variables.** `desktop-audit.sh` keeps counters called `PASS` and
  `FAIL` and its `step` increments them, so a test using the same names had
  its tally silently absorb the assertions. Name the test's counters something
  else.

#### Restoring a file with `mv` can leave you running the old binary

`mv config.rs.bak config.rs` preserves the backup's mtime, which predates the
compile of the version you were experimenting with. Cargo's freshness check is
mtime-based, so it decides nothing changed and reuses the stale artifact: three
consecutive runs reported a failure from code that was no longer on disk, and
the reading of it was wrong in an interesting-sounding way. Restore with `cp`,
or `touch` the file afterwards. The same applies to any revert-to-confirm-red
cycle, which is most of them here.

### Linting
```bash
cargo fmt --all --check
cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings
cd client && npm run lint
```

## Architecture

### Backend (`crates/annex-server/`)
- `lib.rs` exports `app(state) -> Router` and `AppState` struct
- `main.rs` runs the standalone Axum HTTP server via `prepare_server(config)`
- Tests use `tower::ServiceExt::oneshot()` with in-memory SQLite — no real server needed
- Some tests start a real `TcpListener` for WebSocket testing (see `ws_error_handling.rs`)

### Test Patterns
- **Shared harness**: `tests/common/mod.rs` provides `setup_test_app()` and `load_vkey_or_dummy()`
- **In-memory DB**: Tests use `create_pool(":memory:", ...)` — no file cleanup needed
- **Dummy vkey**: When ZK keys aren't available, `generate_dummy_vkey()` is used as fallback
- **Real server tests**: Bind to `127.0.0.1:0` for OS-assigned ports, avoid conflicts

### Key Crates
| Crate | Purpose |
|-------|---------|
| `annex-server` | Axum web server, API endpoints |
| `annex-db` | SQLite database layer, migrations |
| `annex-identity` | ZK identity, Merkle trees |
| `annex-channels` | Channel CRUD, messaging |
| `annex-voice` | Native WebRTC SFU (`webrtc-rs`), Piper TTS, Whisper STT |
| `annex-vrp` | Value Resonance Protocol (trust) |
| `annex-federation` | Server federation |
| `annex-rtx` | Agent knowledge exchange |
| `annex-observe` | Event logging, audit trail |

### Frontend (`client/`)
- Vitest + React Testing Library + jsdom for unit tests
- Playwright for E2E visual tests in `client/e2e/`
- Stores in `src/stores/` (Zustand)
- API client in `src/lib/api.ts`
- ZK proof generation in `src/lib/zk.ts`

### E2E Test Architecture
- Server: `scripts/e2e-server.sh` starts a real Axum server with fresh DB + built client
- Tests: Playwright in `client/e2e/` uses Chromium headless against `http://127.0.0.1:3000`
- Flow: Each test gets a fresh browser context (clean IndexedDB) and goes through the full
  identity creation → server selection → ZK proof → main UI flow
- Startup flow: IdentitySetup (create keys) → StartupModeSelector (use this server) → Chat UI

## ZK Keys

Located at `zk/keys/`. Generated by:
```bash
cd zk && npm ci && node scripts/build-circuits.js && node scripts/setup-groth16.js
```

Tests fall back to `generate_dummy_vkey()` when keys are missing, but some ZK-specific
tests require real keys.

## Desktop crate build

The previously-documented "Tauri API version mismatch" was inaccurate. The
crate is correctly written against Tauri 2.10.x (matching its declared
dependency in `crates/annex-desktop/Cargo.toml`). Two real blockers prevent
a default cargo workspace build from including it:

1. **System libraries** — Tauri 2 on Linux pulls in `gdk-sys`, `gtk-sys`,
   `webkit2gtk-sys`, `soup-sys`, and `pipewire-sys`, each of which expects
   pkg-config dev packages on the build host. `scripts/claude-setup.sh`
   installs the right set (`libgtk-3-dev`, `libwebkit2gtk-4.1-dev`,
   `libsoup-3.0-dev`, `libpipewire-0.3-dev`, `libjavascriptcoregtk-4.1-dev`,
   `libappindicator3-dev`, `librsvg2-dev`, `patchelf`). Without these,
   `cargo check -p annex-desktop` fails in the `gdk-sys` build script with
   "The system library `gdk-3.0` required by crate `gdk-sys` was not found".

2. **Bundle resources** — `crates/annex-desktop/tauri.conf.json` declares
   `bundle.resources = ["../../zk/keys/membership_vkey.json",
   "../../assets/piper", "../../assets/voices"]`. The Tauri build script
   validates these paths at build time. `assets/piper/` and `assets/voices/`
   are gitignored (Piper TTS is downloaded at deploy time), so a fresh
   checkout has no `assets/piper` and the build fails with
   "resource path `../../assets/piper` doesn't exist".

After both fixes (deps installed + `mkdir -p assets/piper assets/voices`)
the crate builds, `cargo clippy -p annex-desktop --all-targets -- -D warnings`
passes clean, and tests compile. CI and full-workspace check commands
must either install those system packages or continue to use
`--exclude annex-desktop` until packaging assets are part of the build
context.

## Desktop CI matrix

The `check-desktop-linux` job in `.github/workflows/ci.yml` is the
canonical validation lane for `annex-desktop`. It installs the GTK /
WebKit / Soup / PipeWire dev libraries and runs, in order:

1. `cargo check -p annex-desktop` — fast Rust-level gate.
2. `cargo clippy -p annex-desktop --all-targets -- -D warnings`.
3. `cargo tauri build --debug` — the full bundle wiring (build-desktop.js,
   frontend, resource validation).

`cargo test -p annex-desktop` is deliberately **NOT** run in that job.
The test build links every Tauri Linux dep (gtk, wry, webkit2gtk) twice
(lib + test binary), which routinely exhausts the standard GitHub runner
disk during the link phase. The release workflow's production tauri
build is the strongest desktop-correctness signal we ship; PR CI's
debug build is the day-to-day gate.

The second desktop job, `desktop-audit`, runs
`bash scripts/desktop-audit.sh` — it takes the bundle past "does it
build" to **does it install and run**: `dpkg -i`, binary on PATH, the
`annex://` scheme handler registered with the OS, a headless Xvfb launch
that survives startup, then `dpkg -r` and confirmed removal. It *does*
attempt `cargo test -p annex-desktop`, but gates it on ~8 GB of free
disk and reports a skip rather than dying mid-link, so a tight runner
degrades instead of failing. See `docs/ui-audit/README.md`.

The `.gitkeep` stubs in `assets/piper/` and `assets/voices/` are
committed so the Tauri resource validator passes on a fresh checkout
without requiring operators to pre-download Piper.

## snarkjs vulnerability containment

The remaining `npm audit` highs in both `client/` and `zk/` come from
the same chain — `snarkjs@0.7.6 → bfj → jsonpath → underscore`. A
build-time scan of `client/dist/assets/*.js` shows **none** of these
package names appear in the production browser bundle: Vite tree-shakes
them out because the proof worker only calls `groth16.fullProve` (a
WASM-backed code path) which doesn't reach bfj's streaming JSON parser.

The chain IS reachable from Node-side tooling (`zk/scripts/test-proofs.js`,
`snarkjs` CLI usage during proof artifact generation) and from any
client code that imports `snarkjs` outside the worker — those are
build-time / dev-time surfaces, not runtime traffic.

The same containment applies to the `circomlibjs → ethers → elliptic`
and `circomlibjs → ws` audit findings: a scan of `client/dist/assets/*.js`
shows no `elliptic`, `ethersproject`, or `secp256k1` traces in the
production bundle. Only the poseidon/blake hashing portions of
circomlibjs are bundled (`client/src/lib/zk.ts` imports just
`buildPoseidon`); the EVM-oriented code paths that pull in ethers are
tree-shaken out.

Replacement path: a follow-up pass should either move to a newer
snarkjs (when upstream drops bfj), or port the proof-generation worker
to a tighter WASM-only entry point that doesn't import the vulnerable
chain transitively. Until then, the chain is documented and
contained, not silently shipped.

## Known Issues

- `annex-desktop`: included in workspace checks when the GTK / WebKit / soup /
  pipewire dev libraries are present AND the gitignored `assets/piper/`,
  `assets/voices/` directories exist (`.gitkeep` stubs are tracked).
  Environments without those still need `--exclude annex-desktop` for
  cargo workspace commands.
