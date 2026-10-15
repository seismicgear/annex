# v0.1.0 public-readiness verification — October 15, 2026

Scope: the application/docs repository, `seismicgear/monolith-annex`, and
<https://monolithannex.com>. The owner's request authorizes publication and
merging the open PRs. Original v0.1.0 artifacts remain unchanged; post-tag source
updates are explicitly unreleased.

## Implementation review

Claims were checked against server routes/startup, desktop configuration and
resource preparation, client identity/invite flows, device certificates,
membership manifests/epochs, private attachments, federation authority/outbox,
relay startup, and speech-provider readiness. The [release ledger](RELEASE_READINESS.md)
separates the signed build and final-source CI from actual native acceptance.

Documentation now distinguishes implemented mechanisms, observed acceptance,
known limitations, and the preserved architectural vision. The roadmap's
unsupported external-review/performance/soak completion claims are reopened.
The website explains joining, local hosting, remote HTTPS, OS trust prompts,
external agent runtimes, and per-channel encryption.

## Checks performed during this pass

- Client: **928 tests / 106 files passed**, plus lint and production TypeScript/Vite
  build after the compatible client patch updates.
- Proofs: all pinned production artifacts verified and installed; real proof
  round trips and tamper/domain-separation checks: **57 passed, zero failed**.
- Rust: strict Clippy across all non-desktop workspace targets passed. Native
  desktop and remaining integration gates are recorded by the pull request's CI;
  a local Linux environment does not qualify Windows or macOS.
- Roadmap: **16 consistency checks passed**; phase summaries and detailed status
  agree, and unfinished criteria remain present.
- Supply chain: exact GLib/Ark backports verified against Cargo metadata; the
  four maintenance dispositions retain reviewed versions, sources, checksums,
  features, parents, and expiration. The proposed WebRTC migration failed a real
  rejoin test and was deferred, so its original bincode disposition remains.
- Published server archive: downloaded, SHA-256 matched the release, extracted
  with original resources, and `--check` passed. The path performs migrations,
  creates an initial channel/signing key, and exits. Missing web/speech payloads
  are now documented accurately.
- Website: invite parser/handler, signed signaling, pinned release catalog, CSP
  nonces, HEAD/404/method behavior tests passed. Browser checks passed at **320,
  390, 768, and 1440 px**, with no horizontal overflow or axe WCAG A/AA findings;
  keyboard skip link, FAQ, destination preservation, and help were exercised.
  Desktop and phone screenshots were visually reviewed.
- **All six installer URLs and SHA256SUMS returned HTTP 200** after redirects.
  Link availability is not a claim that each native installer was executed here.

The website runs its route/signaling/catalog/renderer test gate in the existing
Vercel build. Repeatable browser checks support rerunning them
against the deployed URL. Merge/build/deployment receipts belong to GitHub and
the existing Vercel integration; final production acceptance must inspect the
actual deployed pages, not merely the preview or commit.

Additional browser/backend acceptance: seven startup/full-flow tests passed
against the actual published Linux server archive plus the rebuilt post-tag
web client, including identity, proof, channel join, and messaging. This is not
new native desktop acceptance. Local full Rust execution encountered two real
WebRTC data-channel tests with no gathered candidates in this environment;
those tests remain enabled for CI, not weakened or reported as local passes.
The retained 0.11 stack also cannot establish DTLS in this local environment;
its real-SRTP rejoin acceptance must come from the CI runner.

The anonymous GHCR token request for `seismicgear/annex:pull` returned 403
`DENIED`; production deployment instructions now use a source build or the
public server archive. Package access/visibility remains maintainer-owned work.

## Dependency PR decisions

[Dependency integration](dependency-upgrades.md) records each open PR's result.
TypeScript 7, Arkworks 0.6, and WebRTC 0.17 are explicitly deferred at compatible versions;
merging their histories does not claim those proposed versions were adopted.
No security gate or published ceremony was weakened. Other compatible upgrades
and repository handoff instructions are incorporated. The failed real-SRTP rejoin
check and upstream encoding-lifecycle change are linked in the dependency record.

## License

The owner requested commercial use/modification with a separate reseller/paid
hosting agreement and retained IP ownership. Community License 2.0 implements
that policy. The restriction means it remains source available, not OSI open
source. No copyright assignment or rights to unrelated KI projects are granted.
Contributor ownership and the need for separate broader commercial rights are
explicit. Published binaries receive an additional grant only for rights KI can
license; third-party terms remain intact.

## Remaining work and owner

Maintainer-owned release qualification remains: complete native Windows/Mac
invite/join/message journeys, installed updater acceptance, all speech-provider
combinations, broader network/scale/soak trials, independent cryptographic review,
and a broader alignment corpus. Observable acceptance is a source/artifact-bound
receipt for each path, including failures and skips. The public website does not
claim these are complete.

## Congress handoff

**Not synchronized to KI AI Congress / Constellation.** The connector's orientation
and bounded Annex graph search returned `SOURCE_READ_TIMEOUT`, SQLSTATE `57014`,
phase `nodes`. Its health check reported degraded source reads/worker queue.
The exact repository resolution also returned no acquired repository identity;
that is not evidence that the project does not exist. Repository-document search
found the marketing record but did not establish an existing writable project
thread. No competing tracker or out-of-scope thread was created. This source-linked
repository record preserves the handoff; retry synchronization after the service
and project/thread identity are available. No synchronization success is claimed.

### Native release follow-up — 2026-10-15 20:50 UTC

[PR 322](https://github.com/seismicgear/annex/pull/322) integrates the native
signing correction with main 097261105ef6c00add1fa2dc474151ecf2dee4b1.
The final application candidate is c4513be6f301e49511e219c9c969d5eefd7e109b,
[release run 38800373470](https://github.com/seismicgear/annex/actions/runs/38800373470).
At this checkpoint its Apple Silicon bundle passed strict verification, including
every embedded native file; the other installer jobs were still running. The
earlier bf90d93 packaging probe passed strict verification on both Mac architectures
but was superseded without publication. Native acceptance is a separate gate.

The [Windows v0.1.0 receipt](acceptance/windows-0.1.0-public-invite.json) records
the user's confirmed public invitation launch and explicit correct-server join.
Process, invitation, and server admission evidence corroborate it. It does not
claim a recording of the user's clicks, message exchange, Windows v0.1.1, or Mac
acceptance. Only fresh test identities and the isolated loopback destination were
used. The private invitation code and account paths are omitted from this record.

The [Mac native run](https://github.com/seismicgear/annex/actions/runs/38803891289)
uses the exact candidate with harness ad4e0c8acc28c284e49aee9e4ababb31a9ad16a6.
It creates the founder and invitation through actual UI, follows monolithannex.com,
requires native launch and explicit admission, and correlates an independently
received message with authenticated encrypted storage. Its compiled test server
cache is bound to the exact source, architecture, and binary hash; every session
uses a fresh database. This run was in progress at the checkpoint, not accepted.

The integrated client passed all 928 tests across 106 files. The existing 22
resource/release checks passed, as did CodeQL on the integrated application source.
[Website PR 29](https://github.com/seismicgear/monolith-annex/pull/29) prepares the
v0.1.1 links on the current site design; all 110 route, signaling, catalog and
renderer checks passed. Publication and production link readback remain required.

**Not synchronized to KI AI Congress / Constellation.** The current authorized
workspace orientation also returned SOURCE_READ_TIMEOUT, SQLSTATE 57014, phase
nodes. This existing record preserves the handoff. The owner remains the native
release workstream; the next observable gates are completed native receipts,
unchanged-byte canonical release publication, public download readback, and native
acceptance against the published installers. No new tracker was created.
