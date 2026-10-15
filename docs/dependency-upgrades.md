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
| #312 | Integrate webrtc 0.17.2; remove an obsolete default struct update and re-evaluate its dependency maintenance exposure |
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

Dependabot ignores only these unsupported major transitions, with this rationale.
Compatible/security updates remain enabled. No audit gate, dependency-source pin,
proof verifier, or acceptance requirement is relaxed to merge a PR.
