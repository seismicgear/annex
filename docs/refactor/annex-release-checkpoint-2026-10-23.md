# Annex release checkpoint — 2026-10-23

This is an unfinished release checkpoint, saved at the owner's explicit request to stop additional testing and hand off immediately. It is not a commercial release or a claim that all current installers are verified. Existing GitHub jobs were left running; refresh their results before continuing.

## Start here

- Source and changes: [Annex PR #339](https://github.com/seismicgear/annex/pull/339), branch `fix/annex-commercial-release-20261020`.
- Application/package candidate: `7b118168d2d0a3de51a52ba2da2d3513656c96d0`, version `0.1.3`.
- Engineering checkpoint before this documentation commit: `bcec3cd4bbee708956da43ae4632353091099d5f`. Changes after candidate `7b118168` are workflow infrastructure and documentation only; do not relabel candidate binaries with a different source SHA.
- Local workspace: `C:\Users\monty\Documents\Codex\2026-10-20\take-forge-seismicgear-forge-from-its-2`; repository: `work/annex`.
- Unpublished cloud checkpoint tag: `checkpoint-7b118168-20261023`. Its evidence ZIP contains the current run map, saved status, selected verification receipts, and local acquisition/report helpers. The source is in PR #339; large build artifacts remain in GitHub Actions and any subsequently completed automated dry-run draft.
- Local checkpoint ZIP: `outputs/Annex-checkpoint-7b118168-20261023.zip`. Read its manifest for included files, missing optional receipts, and SHA-256 checksums. No production credentials, private keys, profiles, databases, or dependency directories are included.
- Congress synchronization was unavailable (`SCOPE_UNAVAILABLE`); this repository document and GitHub draft are the handoff.

## Current cloud state at the checkpoint

The saved `work/annex-checkpoint-status.json` supplies the timestamp and individual job results. These observations are a snapshot, not eventual outcomes.

| Work | Run | Checkpoint status |
| --- | --- | --- |
| Candidate packages | [39967168056](https://github.com/seismicgear/annex/actions/runs/39967168056) | In progress. Preflight, Apple Silicon package, server tarball and container jobs passed. Windows, Intel Mac and Linux packages were still running. |
| Current workflow CI | [39969752020](https://github.com/seismicgear/annex/actions/runs/39969752020) | Frontend Tests failed; native Mac XPC boundary passed. Other jobs were still running. Failure cause was not investigated after the stop instruction. |
| Current CodeQL | [39969752049](https://github.com/seismicgear/annex/actions/runs/39969752049) | Passed for JavaScript/TypeScript and Actions. |
| Windows NSIS updater | [39967288060](https://github.com/seismicgear/annex/actions/runs/39967288060) | In progress. |
| Windows MSI updater | [39967293267](https://github.com/seismicgear/annex/actions/runs/39967293267) | In progress. |
| Linux AppImage updater | [39967298707](https://github.com/seismicgear/annex/actions/runs/39967298707) | In progress. |
| Linux DEB updater | [39967304095](https://github.com/seismicgear/annex/actions/runs/39967304095) | In progress. |
| Mac ARM and Intel updaters | [39967282471](https://github.com/seismicgear/annex/actions/runs/39967282471) | Both in progress. |

Current-candidate tests not yet dispatched: Windows NSIS upgrade/repair, Windows MSI upgrade/repair, Linux installation, Mac ARM Host, Mac Intel Host, Mac ARM invite, and Mac Intel invite. The thirteen-case inventory is `work/annex-current-candidate.json`; null run IDs mean not dispatched, not passed.

The successful ARM package is already stored as Actions artifact `annex-macos-aarch64`, artifact ID `12508521391`, 547074384 bytes, archive digest `sha256:3399492865fd14468de230a96fd8db700293da8b64fe7e1e14e32c58e2a3162d`. This is the GitHub artifact archive digest, not the embedded DMG hash. The remaining cloud inventory is saved in `work/annex-checkpoint-build-artifacts.json`.

No complete current-candidate installer set has been acquired or independently qualified. Packaging uses real Tauri updater signatures but `native_signing=false`: Windows packages are unsigned and Mac packages use ad-hoc signing. They are not production publisher-signed downloads. Superseded `875`, `c85`, and `1afe` drafts must not be distributed as current candidates.

## Implemented and verified work

- Fixed the Mac updater's post-swap reply authentication failure. The privileged helper retains the old bundle for the lifetime of the authenticated XPC connection, then releases it on connection invalidation. Caller authentication, package verification, path constraints and app sandboxing remain enforced. See `docs/macos-updater-boundary.md`.
- The real native XPC component regression passed: authenticated caller, unsigned archive rejection, invalid-payload preservation, signed bundle replacement, and authenticated post-swap reply. This does not establish full packaged-app restart/recovery; that remains in the running native updater jobs.
- Updated `ctutils` to `0.4.4` for RUSTSEC-2026-0385. Fresh Rust audit reported zero vulnerabilities and four maintenance notices; no advisory suppression was added.
- Updated the Vitest family to `4.1.12`. Local Node 22 frontend verification passed 1,039 tests in 118 files with four workers and unchanged test timeouts. The newest hosted frontend job failed and must be diagnosed; the local result does not supersede that failure. Audit after the update: zero moderate/high/critical findings, twenty low findings in the full graph and fifteen low production findings.
- Merged the owner's Community License 2.1 and KI Contributor Assignment 1.0 changes. Crate license references match. No assignment was signed for the owner and no invented paid activation mechanism was introduced.
- Fixed Safari invite acceptance navigation to activate Safari, target its actual address field and verify the intended URL. Native execution of that fix is still outstanding.
- Pinned the UI audit to supported Ubuntu 24.04, bounded APT request and lock waits, and added GitHub-hosted-only official HTTPS Ubuntu mirror configuration. The latest mirror change is workflow-only and its running CI remains to be assessed.

Earlier shared-runtime evidence was produced on `47cb8d51ddc77b6f599572a95dd786aa30ce0f59`, not on a relabeled current binary: 1,590 Rust workspace tests (nine ignored), 67 desktop tests (one ignored), two DTLS regressions, two real Whisper provider tests, thirteen core browser cases, five live acceptance lanes, media and lifecycle sessions, and real local-model agent acceptance. `work/annex-current-runtime-equivalence.json` records the comparison to candidate `7b118168`; shared runtime inputs match, with the three changed Mac implementation files excluded and Vitest changes identified as development tooling. Original source labels and receipts are preserved.

Earlier actual `47cb` installer evidence: Windows NSIS/MSI updater passes; Windows NSIS/MSI upgrade and repair passes; ARM Mac Host pass. The old Mac full updater failed after the bundle swap but before restart, which led to the implemented XPC fix. The old ARM invite attempt failed because Safari remained on the wrong page. These historical passes and failures do not constitute current-candidate native acceptance.

Media fixtures used controlled generated audio and a software camera; they are not evidence of physical microphone/camera or independent customer-network acceptance. The direct-transport acceptance used a real introducer and browser with a controlled fixture backend, not a fully deployed customer Store journey.

## Array and Store handoff

- [Array PR #10](https://github.com/seismicgear/Array/pull/10): open, head `7cd75843e77112d5aadee7df34ed932f76f33a8a`. Local Windows desktop suite: 175 passed, one Unix-only skip. Three actual OS-encrypted wallet bridge cases passed with synthetic accounts and production bridge code. Its recorded hosted CI was blocked before execution by the Actions budget.
- [Store PR #370](https://github.com/seismicgear/kuykendall-industries/pull/370): owner closed it unmerged on October 22. Retained implementation branch head `b894ab55457397491d59a919f0eb5954a33d666f`; 147 Node 22 tests passed. Current inspected Store main `6e027a0825b973d9eb53f8f8ac19b9611383079d` does not include the intake implementation. Do not reopen the owner's closed PR automatically.
- The Store implementation follows the actual Vercel/Supabase architecture and reuses Array's product-scoped verifier, serialization, replay/downgrade/equivocation protections, withdrawal and audited database writes. It is tested integration source, not deployed production commerce.
- Local related checkouts: `work/array-annex-integration` and `work/ki-annex-integration`.

## Remaining work, in execution order

1. Refresh the existing runs above. Diagnose the new frontend CI failure before treating CI as green; inspect logs rather than assuming the earlier local pass covers it.
2. Collect completed candidate packages and signed updater metadata from build `39967168056`. Confirm actual source SHA, platform inventory, file hashes and updater signatures. If the build fails, fix it and build a newly identified candidate; never substitute old artifacts silently.
3. Finish the current native updater cases and dispatch the seven missing native acceptance cases against the exact candidate artifacts. Give special attention to Mac post-swap authenticated reply, process restart/recovery, Safari invite navigation and Linux installation/update paths.
4. Complete exact artifact acquisition and qualification receipts. The saved helpers provide the acquisition/report pipeline. They are workspace utilities, not evidence by themselves; regenerate them from `annex-current-candidate.json` if IDs change. `write-current-annex-delivery-report.py` is only appropriate after its native and CI gates pass. Do not run it to manufacture a finished report for this checkpoint.
5. Resolve the Array hosted-CI budget block and the owner's Store intake disposition; deploy the accepted implementation with real product-scoped release trust and verify a complete Store -> Array -> install -> launch -> update -> offline recovery -> repair journey.
6. Produce publisher-signed and notarized production installers and qualify normal downloaded installs, updates and recovery before a commercial release. Verify supported minimum operating-system claims, physical-device sessions and independent-network acceptance separately.

The evidence ZIP preserves current acquisition helpers and their older template dependencies. On another machine, adjust workspace paths and supply authenticated GitHub CLI access; the local authentication wrapper is intentionally excluded. The original local wrapper is at `C:\Users\monty\Documents\Codex\2026-10-15\build-array-and-deliver-the-packaged\work\github-auth.ps1`. Never paste credentials into a report or repository. Large GitHub Actions artifacts have retention limits; acquire them before expiry. Existing cloud jobs may have completed since this checkpoint.

## External commercial release prerequisites

- Windows publisher signing certificate/PFX, password, expected certificate thumbprint and trusted timestamp URL. The local signing certificate inventory was empty.
- Apple Developer ID certificate/P12 and password, signing identity/team, and notarization authentication. Ad-hoc signing does not satisfy Gatekeeper distribution.
- Product-scoped `ANNEX_ARRAY_RELEASE_PRIVATE_KEY` and key ID, approved download hosts, Windows publisher thumbprint and Apple team ID. Repository configuration inspected at this checkpoint supplied Tauri updater keys only.
- Accepted and deployed Store intake, scoped `SUPABASE_DB_URL`, verified project reference and authorized deployment actor/apply procedure. No production database write was performed for this handoff.
- A production updater origin and a verified actual customer acquisition/recovery workflow.
- Community License 2.1 remains the actual licensing model. Resale/paid-hosting permissions require the separate written agreement described by that license; no paid/member activation gate is represented as completed.

Ready now: reviewable committed implementation, saved verification evidence, and the successful platform artifacts already in GitHub Actions for further qualification. Not ready: a complete verified current installer set or a commercially releasable product.
