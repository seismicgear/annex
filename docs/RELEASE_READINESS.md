# Release Readiness & Packaging Proof

This document preserves historical qualification records and describes reproducible
release checks. A runnable command or a configured workflow is not an execution
receipt. Historical results below do not qualify later source, another operating
system, or an installed application.

## Current 0.1.3 candidate — October 20, 2026

The exact native candidate is `87550094627d8c84ca001bbcb3623f09f7ea0794`,
submitted in [PR #339](https://github.com/seismicgear/annex/pull/339).
It supersedes c85f710 and is not a public commercial release. Native qualification
found and repaired an installed-resource precedence defect: a build checkout
could shadow bundled keys, and macOS correctly denied access from the sandbox.
Installed keys, Piper, voices and alignment resources now take precedence.
The replacement package evidence below is bound to 87550094; earlier package
receipts do not qualify these bytes.

| Scope | Verified evidence | Limit |
| --- | --- | --- |
| Native build, inventory and upload | [39467676029](https://github.com/seismicgear/annex/actions/runs/39467676029) passed every required native/server/preflight job, signature and SBOM assembly, and exact upload of 55 assets into draft `418685781` | The release remains unpublished; updater signatures are independent of OS publisher trust |
| Windows x64 EXE and MSI | [39473252886](https://github.com/seismicgear/annex/actions/runs/39473252886): public 0.1.2 install, actual founder and message, candidate upgrade, forced-stop/reopen recovery; installed binary hashes match the appropriate format's inventory | Isolated profiles deliberately disable in-app replacement; this qualifies native installer upgrade and recovery |
| Linux x64 DEB and AppImage | [39474215545](https://github.com/seismicgear/annex/actions/runs/39474215545): installed native UI, founder/message and reopening; DEB uninstall preserves profiles and reinstall recovers the message | No physical media or external native invitation claim from this run |
| macOS native Host | [ARM 39471087867](https://github.com/seismicgear/annex/actions/runs/39471087867), [Intel 39472567707](https://github.com/seismicgear/annex/actions/runs/39472567707): one Host action creates the founder, copied public invitation opens its real page, independent direct WebRTC summary matches, and founder/server/public address survive reopening | Ad-hoc signatures; ordinary browser-download/Gatekeeper acceptance is not covered |
| macOS native invitation | ARM [39471095278](https://github.com/seismicgear/annex/actions/runs/39471095278) and Intel [39472574363](https://github.com/seismicgear/annex/actions/runs/39472574363) passed exact DMG installation, Safari invitation, explicit join, independently received message and same-process warm invitation | Uses a separately provisioned reachable HTTPS destination; Host and independent recipient journeys are separate tests |
| Linux AppImage in-app update | [39471081130](https://github.com/seismicgear/annex/actions/runs/39471081130) passed stalled-transfer recovery, corrupt-signature rejection, successful retry, exact native replacement, automatic restart and founder/message retention | Controlled HTTPS origin and labeled test baseline; the recovery observer reopened the verified executable after independently observing its actual restarted process |
| Linux DEB in-app update | [39475380938](https://github.com/seismicgear/annex/actions/runs/39475380938) passed stalled-transfer recovery, corrupt-signature rejection, verified retry, actual package replacement, automatic restart and ordinary founder/message recovery | Controlled HTTPS origin and labeled baseline; privileged installation used the disposable runner's existing authority, not an exercised password prompt; the recovery observer reopened after observing the real restart |
| Windows in-app updates | [EXE 39471633279](https://github.com/seismicgear/annex/actions/runs/39471633279) and [MSI 39472176568](https://github.com/seismicgear/annex/actions/runs/39472176568) passed stalled/corrupted downloads, signature-verified retry, actual installer replacement, automatic restart and ordinary founder/message recovery | Controlled HTTPS origin and labeled test baseline; production-origin and Authenticode acceptance remain separate |

Mac native in-app updater fault/retry/replacement/restart acceptance is under
qualification in [39477741199](https://github.com/seismicgear/annex/actions/runs/39477741199).
The native tests use ordinary profiles and an explicitly labeled 0.1.2 test
baseline built from the same application source, with only version and HTTPS
fixture endpoint overrides. It uses the production verification public key and
these exact signed candidate target packages; it does not use a private signing
key in the app or claim to be the public 0.1.2 binary. Disposable-runner TLS trust
is recorded and cleaned up; production-origin service operation remains separate.

All 55 release assets were acquired locally from the authenticated assembly
artifact `12286754997`, whose SHA-256 is
`ed6f5372ca7f828cb0ca7a6a769e93ec89c8177600d04f1c2c489dfa9d5719f8`.
Every extracted file matches the release checksum inventory; all local sizes and
SHA-256 digests also match the 55 remote draft assets (2,697,119,561 bytes).
No downloaded installer was rebuilt or modified for delivery.

The stalled-download repair has real HTTP coverage for no response bytes,
partially delivered responses and slow successful streams. The first native Linux
updater attempt independently rejected stalled and corrupted transfers, verified
the valid payload and replaced the AppImage. Its restart observer compared the
mounted ELF to Array's whole-AppImage hash; that observer was corrected to derive
the ELF hash from the authenticated candidate. The complete corrected rerun passed,
including the replacement source linked above. The real restarted process was observed before a fresh WebKit
observer reopened the verified program to inspect the persisted ordinary profile.

The application source also passed 1,585 Rust workspace tests (9 ignored),
67 desktop tests (1 ignored), 1,039 client tests, frontend lint/build, 13 ZK
artifact tests, release-tooling tests, and real browser transport/security workflows.
Controlled live acceptance includes two independently decoded audio/video peers,
screen sharing, mute/deafen, Whisper captions, disconnect/rejoin/takeover/revocation,
and an actual Ollama/Piper/Whisper agent with fresh-message and voice checks.
Those receipts establish the exercised providers and controlled inputs, not every
physical device, network topology, or load profile. Runtime tests and native
package acceptance remain separately recorded.

Two normally ignored Whisper provider tests were additionally executed with the
real packaged binary/model and a labeled Piper-generated known-speech waveform;
both passed, including mid-utterance room-tap context. They do not establish
arbitrary microphone or language accuracy. The normally ignored native direct
transport test also passed against the deployed Monolith introducer with real
native peers and Chromium 141: authenticated HTTP, WebSocket messages, 1 MiB
file integrity, and continued browser traffic after further introductions were
blocked. Its backend is an explicit test fixture; the production introducer was
not stopped. This source test does not replace installed-package acceptance.

The broad runtime receipts use server source
`343b4874195865f869b357e9352f6b1e2c1fca2a` and executable SHA-256
`008843e768599b8601d0fb2a8c555ebd267ca1873c5a1f88970815c9dd12ae8d`.
Its server, client, Cargo and vendored application source is unchanged at this
candidate; the later desktop resource-selection repair has its own 67-test run
and the exact installed-package checks above. Qualification-only commits after
87550094 are distinct from the candidate's compiled application source.

The superseded c85f710 candidate remains an unpublished draft (ID `418644113`),
explicitly labeled superseded. Its recovered 55 assets were matched by size and
SHA-256 in [39466157468](https://github.com/seismicgear/annex/actions/runs/39466157468).
Its prior native and updater results remain historical evidence only.

### Ecosystem and publication gates

- [Array #10](https://github.com/seismicgear/Array/pull/10) resolves real quoted
  Windows installation paths while retaining identity and publisher checks.
  The native Windows desktop suite passed 175 tests with one Unix-only skip.
  The Annex identity bridge also passed against Array's actual broker and
  OS-encrypted wallet, including recovery, offline behavior and lost acknowledgement.
- [KI Store #370](https://github.com/seismicgear/kuykendall-industries/pull/370)
  implements authenticated signed-release intake in the actual KI Vercel/Supabase
  architecture. Stream serialization, replay/downgrade/equivocation/withdrawal
  checks, audited writes and revoked-download rejection passed 147 local tests.
  Array's retained `services/store` is reference code, not the production owner.
- Array and Store CI could not start because GitHub reported that Actions budgets
  prevented execution. Their local results do not replace those remote gates.
  No live production Store trust, offer, entitlement or release record was changed.
- Windows Authenticode and Apple Developer ID/notarization credentials are absent.
  [Native signing](native-release-signing.md) requires the real certificates and
  publisher pins; it fails closed when they are missing. Newly signed bytes must
  be inventoried and qualified again.
- Production Array release keys, allowed hosts and publisher policies must be
  provisioned, then the real Store/Array/customer download and install journey
  must pass. [The distribution contract](architecture/array-distribution.md)
  identifies the exact owner, commands, policies and licensing boundary.
- Annex Community License 2.0 supplies the current use grant. Annex has no
  invented paid activation gate; paid hosting/resale requires the existing
  separate written agreement. Broad network/load qualification, optional provider
  coverage and independent cryptographic review remain distinct limits.

## Historical published v0.1.2 and updater repair baseline

[v0.1.2](https://github.com/seismicgear/annex/releases/tag/v0.1.2) is public from
`e23b3ac3334a5fa28a07d581dfc420cd0505e75c`. Its original
[signed build](https://github.com/seismicgear/annex/actions/runs/39096151500)
passed. All 50 release assets were hashed and updater signatures verified;
publication retained the native payload bytes and changed only release metadata.
The Monolith production site serves this release.

The installed Linux DEB created its founder and public invitation automatically,
answered an independent direct WebRTC summary request and retained its server,
identity, address and stored message after reopening. The Windows installation
was repaired with the verified v0.1.2 installer after an interrupted update left
a truncated executable. Its original founder, server and stored message remained
unchanged, and its generated invitation uses Monolith. The interrupted updater
is a failure, not a passing in-place update receipt.

Both actual Mac architectures passed native invitation admission, independently
received messaging and warm protocol delivery: [Apple Silicon](https://github.com/seismicgear/annex/actions/runs/39098657653)
and [Intel public installer](https://github.com/seismicgear/annex/actions/runs/39099967852).
The destination in those runs was a separately provisioned reachable HTTPS
server. Mac Host permission handling and external Mac-to-Windows/Linux direct
connectivity remain under repair; these successful recipient runs do not qualify
those separate paths. Native package signatures retain the limitations below.

The v0.1.3 source bounds stalled downloads without imposing a total time
limit on progressing transfers. Real HTTP tests exercise both a silent server
and a partially delivered response, plus a slow successful response; UI tests
verify retry without exposing unverified installation. A released installer and
an actual installed update are still required to qualify that repair.

## Published v0.1.1 — separate macOS packaging correction

The records below describe the historical versions. The published v0.1.2
uses invitation/introduction-only Monolith routing and direct established
connections; historical opaque-relay descriptions do not describe its transport.
October 17 local acceptance exercised real HTTP and direct WebRTC federation,
signed human admission, encrypted device recovery, bidirectional attachment
transfer and deletion after the final remote member was revoked. The five-server
direct WebRTC mesh also passed its signed hop, cycle, redaction and removal checks.
These local source checks do not independently qualify installer bytes.

[v0.1.1](https://github.com/seismicgear/annex/releases/tag/v0.1.1) was published
at 22:07:05 UTC on October 15, 2026, from
`c4513be6f301e49511e219c9c969d5eefd7e109b`. Its 45 assets are separate from the
unchanged v0.1.0 release. The installation guide linked v0.1.1 at that time.

| Evidence | What it establishes | Boundary |
| --- | --- | --- |
| [Native release build](https://github.com/seismicgear/annex/actions/runs/38800373470) | All native/server/preflight jobs and signature assembly passed, including strict full Mac bundle and embedded-code sealing | GitHub upload failed; a build alone is not native UI acceptance |
| [Verified assembly recovery](https://github.com/seismicgear/annex/actions/runs/38809512187) | Original artifact hashes, provenance and production updater signatures were verified; native payloads were not rebuilt | Publication was a separate authorized operator action |
| [Published Mac sessions](https://github.com/seismicgear/annex/actions/runs/38814938771) | Both Apple Silicon and Intel: exact published DMG → installed app → Safari public invite → explicit join → independent recipient's message → same-process warm invite; UI, HTTP and database records agree | Receipts explicitly say `published_installer: true`, `notarized: false`, and `browser_download_gatekeeper_acceptance: false` |
| Windows/Linux v0.1.1 | Native builds and package gates passed | Full installed invitation/message journeys for these exact newer packages remain unqualified; v0.1.0 results below do not transfer |
| Published Linux server v0.1.1 | Downloaded archive matches SHA-256 `fd3e14559c51560dbe73fc66b2c97d5ab1d6c6c2b562a3c8d7216190aa0b60d4`; documented production `--check` succeeds | Startup/migration validation only; the archive has no browser client or speech runtime |

Published Mac receipt SHA-256 values: Apple Silicon
`48cd8855a8b5e810f6844859cc7b5eff0790f00a36bd9f55372f1eed14bbd61f`;
Intel `9efb42e1d4a087cf6e743abbed943f29e688be55c9f06e18e818feec1a04a046`.
Mac signatures are ad-hoc; Apple Developer ID/notarization and ordinary
browser-download/Gatekeeper acceptance remain absent or unqualified. Windows
installers are not Authenticode-signed. Installed updater replacement, complete
speech/provider coverage, broad NAT/load qualification, and independent
cryptographic review remain open.

The separate [ordinary Host probe](https://github.com/seismicgear/annex/actions/runs/38817042173)
failed on both published Mac installers. Actual UI reached the local server's
Admin and Server Settings screens, but public endpoint registration failed and
no public Copy Link was available. The probe's database observer found no database
at its assumed path, so its founder-row assertion does not establish a product
admission failure. A separate identity-creation click was required. This is not a
passing zero-configuration public-hosting receipt. The successful invitation and
message runs above used an independently configured, reachable destination.

Post-release source also fixes Host startup restarting on a parent rerender and
adds bounded discovery/identity request deadlines. Those fixes require a later
installer release; the published v0.1.1 bytes do not contain them.

The additional [Host/founder repair](https://github.com/seismicgear/annex/commit/6444bfbde5a90df119fb036112ea0a38026d1120)
automates identity creation from Host, selects the existing founder by the actual
running database, recovers interrupted admission from matching saved secret keys,
and establishes membership before public-endpoint acquisition. Startup no longer
erases the server when browser keys and the onboarding marker are absent. The
profile badge comes from the server's founder identity, not moderator capability.
Local verification: 949 client tests, lint, production frontend build, strict
desktop/server Clippy, two Windows-compiled desktop database tests, and the backend
identity endpoint test passed. These are implementation checks, **not a repaired
published Windows/macOS Host receipt**. The native Host observer now identifies
the exact installed process and reads only its actual open database.

[Diagnostic run 38820552079](https://github.com/seismicgear/annex/actions/runs/38820552079)
resolved the observer gap on both published Mac architectures. Each actual app
created one active HUMAN identity with moderator and invite permissions. Its
database was inside the macOS application container. Each server's public URL
remained loopback HTTP, router acquisition failed in the UI, and Copy Link never
appeared. Both tests failed only at that public-invite step. An extra explicit
identity-creation click was also still necessary. This confirms successful local
founder creation on Mac and failed automatic public hosting; it does not qualify
Windows, the unreleased repair, or one-click setup. The installed payloads still
match c4513be and their strict ad-hoc signature verification passed.

In those v0.1.1 runs, the obsolete registration target was unavailable and
automatic public hosting failed. This historical failure does not describe the
v0.1.2 transport: Monolith now performs content-blind introduction, followed by
direct peer connections, as described in the
[deployment notes](deployment.md#desktop-host-mode). The old installed bytes
remain useful only as an explicitly identified upgrade baseline.

## Published v0.1.0 — October 15, 2026

The [public release](https://github.com/seismicgear/annex/releases/tag/v0.1.0)
was published at 17:35:51 UTC from
[`2ac5d190481757ce871a3149feafc6f3c44b2b47`](https://github.com/seismicgear/annex/commit/2ac5d190481757ce871a3149feafc6f3c44b2b47).
The original tag, binaries, signatures, and checksums remain unchanged.

| Evidence | What it establishes | Boundary |
| --- | --- | --- |
| [Signed release workflow](https://github.com/seismicgear/annex/actions/runs/38769507686) | Four required native targets, desktop resource inventories, artifact/signature checks, Linux server tarball, container and startup checks passed | A successful build is not full native user acceptance |
| [Release-source CI](https://github.com/seismicgear/annex/actions/runs/38769518406) | Server/client checks, desktop build/audit lanes, UI audit, and Linux/Windows server smoke passed | PR macOS lane is conditional; actual macOS release builds ran in the release workflow |
| [CodeQL](https://github.com/seismicgear/annex/actions/runs/38769516895) | Static analysis passed on release source | Not an independent security or cryptographic audit |
| [Published native acceptance account](https://github.com/seismicgear/annex/releases/tag/v0.1.0) | Real Ubuntu session with the published DEB: app-created public invite → OS handler → identity creation → explicit Join → intended server → independently received message. Repeated invite stayed in the same process | Linux DEB acceptance; not transferred to Windows or macOS |
| Same release account, AppImage | Production-signed AppImage launched from a persistent path containing spaces; first launch registered its handler; invitation reached the same process without another admission | Launch/handler acceptance, not an independently repeated full message journey |
| Same release account, final-source browser runs | 426 checks across 105 surfaces, zero findings; 13 functional browser tests; 6 group-call tests | Browser/fake-media evidence is not native microphone, display-capture, or every network topology |

The published installer catalog contains Windows x64 EXE/MSI, Linux x64
AppImage/DEB, and Apple Silicon/Intel DMGs. `SHA256SUMS`, `downloads.json`,
`latest.json`, updater payloads/signatures, SBOMs, ceremony manifests/transcript,
and dependency-backport provenance accompany them. See [Installation](installation.md).

The server container was built and pushed by the release job, but anonymous
GHCR pull authorization returned HTTP 403 `DENIED` during the public-readiness
pass. [Deployment](deployment.md) therefore leads with a production build from
the published tag and offers the public server archive; anonymous container
availability remains unresolved.

### Still incomplete

- Native Windows and macOS invitation-to-message acceptance. An earlier Windows
  Sandbox attempt reached the executable but WebView2 could not initialize or
  complete repair; no Windows join/message pass is inferred from that attempt.
- Actual installed updater replacement, all optional speech-provider/platform
  combinations, and broader media/network/NAT qualification.
- Automatic public hosting/tunneling. The invitation site only routes a link;
  the destination must supply reachable HTTPS. Signaling is a separate service.
- Independent comprehensive security/cryptographic review, a multi-contributor
  ceremony, broader alignment-threshold calibration, measured target-scale/latency
  qualification, and a seven-day load soak.

Windows binaries are not Authenticode-signed. A later packaging investigation
in [#322](https://github.com/seismicgear/annex/pull/322) found **incomplete macOS
bundle signatures in v0.1.0**. Earlier build/updater checks did not establish
complete native bundle sealing. Use the separate v0.1.1 correction documented
above; Developer ID signing and notarization remain unavailable.
Tauri updater signing is a separate trust mechanism. The
roadmap retains these incomplete requirements rather than treating publication
as evidence of their completion.

### macOS correction after publication

PR #322 adds explicit ad-hoc signing, signs the embedded Whisper executable
before resource provenance is recorded, and fails release upload unless the
finished app and every native Mach-O file pass strict verification. Its first
candidate was rejected because the Intel Whisper binary was unsigned. The
[corrected candidate build](https://github.com/seismicgear/annex/actions/runs/38795731644)
used source `bf90d930663a14b3a1db7f24e0d741ed3475675d` as a diagnostic candidate
that was superseded without publication. The final c4513be build and the published
Mac acceptance receipts are recorded above.

The new native workflow binds the exact downloaded DMG and destination server
to the release source and checks Safari → installed Annex → explicit Join →
independently received message, plus warm invite delivery. Its receipts explicitly
do not qualify browser-download quarantine/Gatekeeper behavior. Successful
execution is established only by its own source/artifact-bound receipt, such as
the published v0.1.1 Mac sessions above.

The same PR reports a user-performed Windows v0.1.0 public-invite launch and
correct-server admission, corroborated by the installed process and admission
observations. This narrower evidence does not qualify a complete Windows message
journey or any v0.1.1 Windows installer.

### Public-presence pass

[October 15 public-readiness verification](public-readiness-2026-10-15.md)
records the later website/documentation/license and dependency-integration work.
That work is on `main`; it does not retroactively alter the release. In this
pass, the actual server archive's SHA-256 was checked against the release digest
`2ef062e9102fa57e9611adefba2a3189a15757e27bda2f6669333e82c3add645`, and its
`--check` startup path succeeded. The archive contains neither `client/dist`
nor speech executables/models; those require separate operator setup.

## Historical local checkpoint — October 13, 2026

The following paragraphs and later historical inventories record earlier
contexts. They do not supersede the published evidence above.


The repaired Windows qualification run recorded 1,560 Rust workspace tests passed,
zero failures and nine explicit ignored entries; the separate desktop suite passed
41 tests with none ignored. The client passed 897 tests in 102 files. Strict Rust
and desktop Clippy, formatting, client lint/type checks and production builds
passed. The frozen Windows server SHA-256 is
`7BD3B4CC3EA41DEFB1CD44022220D93A3869C600A97352505C3A7DCF319A4E3B`;
the full-resource desktop candidate is
`B97431BBE5C1812B5EE19BA9D448F4FDB4DC259424F0BF8B29C83004CE972DF8`.

These are local build/component results. Current installed Windows/Linux/macOS
install–launch–update–uninstall, actual OS deep links, new cross-server authority
and attachment journeys, and public hosting still have separate acceptance gates.
Two actual Windows IndexedDB-lock recovery journeys passed on the new client with
the prior server; that narrow run made no write API calls. No current GitHub CI
execution or production deployment is asserted here.

## Historical automated test inventory (September 2026)

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

The counts above were recorded for the historical sweep below, not the current
checkout. They are not current targets or evidence for unexecuted lanes. If a
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

### Recorded historical sweep behind these numbers

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
> runs 751 through 756 on
> `claude/beautiful-bohr-9qb34p` — five `workflow_dispatch` runs and the
> `pull_request` run for #296, eight attempts counting two re-runs, over about
> four hours — ALL died in three to ten seconds: five jobs each time,
> `ubuntu-latest` and `windows-latest` alike, `runner_id: 0`, `runner_name`
> empty, and HTTP 404 for the job logs because no log was ever written. The five
> jobs that `needs` them were skipped, so each run reports as a failure with
> nine of ten jobs having executed nothing.
>
> It is not branch-specific and it did not end with the merge. `main` at
> `622534e` — the merge of #296 — got run 757, which died in six seconds the
> same way, and the CodeQL workflow (`Analyze (actions)`,
> `Analyze (javascript-typescript)`) does too. So **that historical branch state had
> no CI result at all**, on any branch, and nothing here should be read as one —
> the red checks on #296 and on `main` are this, not the diff.
>
> The remedy is on the account rather than in any branch: an Actions spending
> limit reached, or a runner/workflow permission policy. Until it is fixed, a
> merge to `main` buys no verification, so the local sweep above is the whole of
> the evidence — Linux only, and silent about Windows and macOS.
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

Run `cargo tauri build` from `crates/annex-desktop` after installing the
Tauri CLI and platform build prerequisites. The configured bundle targets are:

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

### Historical Linux install → run → uninstall procedure

```bash
cd crates/annex-desktop
SKIP_PIPER=1 ANNEX_BUILD_PROFILE=dev cargo tauri build --bundles deb   # build (Piper staged via scripts/setup-piper.sh)
sudo dpkg -i ../../target/release/bundle/deb/Annex_*.deb               # install — binary on PATH, annex:// handler registered
xvfb-run -a annex-desktop                                             # launch — WebView loads the React frontend
sudo dpkg -r annex                                                    # uninstall — binary, .desktop handler, /usr/lib/Annex all removed
```

The September Linux sweep above records execution of this cycle at `f491be8`.
That is retained historical evidence, not a current installed-app result. The
current Windows native candidate has been built locally; its installed lifecycle
and signed updater still require actual execution. A workflow definition for a
Windows runner does not establish that an installer was built or run. Each release
needs its own commit, binary hashes, runner logs and installed-session receipt.

### Downloadable installer artifacts

`ci.yml` defines installer build jobs; only completed job logs establish that a
specific platform built. Those jobs do not upload installers. To get installable artifacts you can download (and actually run on
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

Intended end-to-end path, with encoder/parser contract checks recorded in both
repositories. Current hosted invite redemption and installed cold/warm OS dispatch
require a real session in addition to these component checks:

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
cd client && npm run test:e2e               # run the current Playwright functional suite
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
