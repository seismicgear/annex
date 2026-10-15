# Dependency integration after v0.1.0

These decisions concern `main` after the published tag. They do not change the
v0.1.0 binaries or ceremony. The public-readiness integration includes the
original open PR histories and explicit compatibility corrections, rather than
representing every proposed version as adopted.

| PR | Resolution |
| --- | --- |
| #299, #315, #316 | Integrate checkout 7, download-artifact 8, setup-node 7; resolve their overlapping CI edits |
| #313 | Integrate the tokio-tungstenite development dependency update |
| #320 | Integrate the client patch updates and regenerate the supported lock graph |
| #312 | **Defer webrtc 0.17.2.** Retain 0.11: the proposed upgrade breaks publication rejoining over real SRTP; the unchanged regression test caught it in CI |
| #309 | Integrate hkdf 0.13 using an explicit SHA-2 0.11 alias for HKDF; retain the SHA-2 0.10 API for unrelated HMAC/hashes; existing cross-language known-answer tests protect the wire derivation |
| #310, #314 | **Defer Arkworks 0.6 version bumps.** Retain Groth16 0.5 and SNARK 0.5.1 with the coherent BN254/FF/EC/serialize stack, light-poseidon 0.4, and reviewed ark-relations backport |
| #302 | **Defer TypeScript 7.** Retain TypeScript 5.9: current typescript-eslint 8.71.2 declares TypeScript `>=4.8.4 <6.1.0`; the proposed installation fails peer resolution |
| #318 | Integrate repository handoff instructions; retain the owner's task authority and privacy boundaries |

The Arkworks PRs independently introduce incompatible 0.5/0.6 field/pairing
traits. Upgrading those traits, the Poseidon dependency, audited relation crate,
and proof fixtures requires a coherent cryptographic migration and full real
proof/negative-path acceptance. Changing only Groth16/SNARK is not such a migration.
The current published light-poseidon 0.4.1 still depends on Arkworks 0.5.

The TypeScript deferral is based on the npm registry's actual peer-dependency
metadata, not a suppressed warning. Revisit when the parser/linter supports the
compiler and the build/test configurations are migrated. Do not use
`--legacy-peer-deps` or disable type checking as a resolution.

The WebRTC proposal compiled, but [CI](https://github.com/seismicgear/annex/actions/runs/38788361608)
failed `rejoined_publication_preserves_sequence_over_real_srtp` with
`ErrRTPSenderNewTrackHasIncorrectEnvelope`. In 0.17.2,
`replace_track(None)` removes the encoding required by a subsequent
`replace_track(Some(...))`. Annex parks and reuses publication slots to preserve
SSRC, sequence, timestamps, and the browser receive pipeline. Adopting this
version needs a deliberate lifecycle migration and real group-call rejoin
acceptance; do not suppress the test or silently create new slots. The original
reviewed bincode maintenance disposition remains because the 0.11 graph still
contains it.

Dependabot ignores these unsupported transitions, with this rationale.
Security updates remain enabled; routine version proposals are temporarily paused as described below. No audit gate, dependency-source pin,
proof verifier, or acceptance requirement is relaxed to merge a PR.

## Release stabilization after the first merge

Dependabot refilled its queue immediately after the first integration. The next
batch is resolved explicitly while the macOS packaging correction is qualified:

| PR | Resolution |
| --- | --- |
| #323, #324, #325, #331 | Integrate release action 3, build-push action 7, upload-artifact 7, and setup-buildx action 4; retain existing inputs, evidence uploads, and immutable release policy |
| #326–#330 | Defer globals 17, TypeScript 6, react-refresh 0.5, React/DOM 19.3, and Node 26 types. Preserve the frontend graph verified by the readiness CI. These are unqualified upgrades, not a claim that every proposal is incompatible |
| #332 | Defer the Cargo patch group. It changes the explicit `rust-ini =0.21.1` Linux compatibility pin to 0.21.3 even though tauri-plugin-deep-link 2.6.2 initializes the old two-field `ParseOption`. Split compatible patches from that migration before revisiting |
| #333–#336 | Defer jsonwebtoken 11, HMAC 0.13, dirs 7, and tower-http 0.7 pending their own API/security/installation qualification. Keep the tested JWT, SHA-2/HMAC, profile paths, and middleware behavior during the packaging correction |

The integration retains each proposal's history with its documented resolution;
it does not represent a deferred dependency version as installed. None of these
PRs identifies a security advisory requiring its proposed upgrade. Existing
supply-chain gates and reviewed, expiring dispositions remain in force.

**Temporary policy:** `open-pull-requests-limit: 0` pauses ordinary version PRs
for the four ecosystems. GitHub security updates have a separate limit and are
not disabled by this setting ([GitHub documentation](https://docs.github.com/en/code-security/reference/supply-chain-security/dependabot-options-reference#open-pull-requests-limit)).
The maintainer should restore Cargo **5**, client npm **5**, ZK npm **3**, and
GitHub Actions **3** after the packaging correction is qualified, with a review
no later than **October 22, 2026**. This is a review deadline, not an automatic
scheduled change. Keep the specific Arkworks/WebRTC/TypeScript-7 exclusions until
their documented migration requirements are met.
