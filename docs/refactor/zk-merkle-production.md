# ZK + Merkle: Current vs. Production Target

This document describes the current state of Annex's identity ZK and Merkle
machinery, and the deltas required for a production-grade deployment.
"Current" means "what is in this branch right now". "Target" is what must
exist before a public release.

The reader is assumed to know that production-grade requires:
- A trusted setup ceremony (or a Plonk-style universal setup) with
  documented, auditable contributions.
- Stable artifact identifiers shipped with every binary.
- A coherent epoch model so that root rotations don't break clients.

---

## Current state

### Membership circuit (`zk/circuits/membership.circom`)

- Hash: Poseidon over BN254. Implemented via `circomlib/circuits/poseidon.circom`.
- Identity commitment: `commitment = Poseidon(sk, roleCode, nodeId)` (3 inputs, 1 output).
- Tree depth: **20** (`component main = Membership(20);`). Capacity = 2^20 = 1,048,576 leaves.
- Membership template recomputes the commitment internally from `(sk, roleCode, nodeId)` then walks `pathElements[20]` and `pathIndexBits[20]` to produce a `root`. A `Num2Bits(20)` constraint binds `leafIndex` to `pathIndexBits` so the proof's claimed index matches the path it traversed.
- Public outputs: `[root, commitment]`. **Order is load-bearing** for the verifier in `crates/annex-server/src/middleware.rs`.
- Stats from `node scripts/build-circuits.js`: 5,184 non-linear constraints, 5,822 linear, 11,030 wires.

### Identity circuit (`zk/circuits/identity.circom`)

- Single Poseidon-3 hash; emits the same commitment value used as a leaf in
  `membership.circom`. 264 non-linear constraints. Currently used for
  client-side commitment derivation; not the path used at server-side
  authentication.

### Trusted setup tooling (`zk/scripts/setup-groth16.js`)

- Powers of Tau: depth-14 (`pot14_*.ptau`) — sufficient for the 5,184-constraint membership circuit but **not for further expansion**. If a circuit grows past ~16k constraints, the script will need a depth-15+ PoT.
- Generates per-circuit `*_0.zkey`, `*_final.zkey`, `*_vkey.json` via `snarkjs groth16 setup` + `zkey contribute` + `zkey export verificationkey`.
- Entropy is sourced from `crypto.randomBytes(32).toString("hex")` per contribution. **The current setup runs unattended in CI** — there is no recorded multi-party ceremony, no contributor manifest, no hash chain across contributions.

### Verification key shipping

- File: `zk/keys/membership_vkey.json`.
- Bundled via `crates/annex-desktop/tauri.conf.json::bundle::resources` as `../../zk/keys/membership_vkey.json` (Tauri resolves relative to tauri.conf.json's directory).
- Server load path: `crates/annex-server/src/lib.rs` reads it on boot, parses with `annex_identity::zk::parse_verification_key`, falls back to `generate_dummy_vkey()` with a `tracing::warn!` if missing.
- The dummy fallback is **dev-only** — see I-ZK-2 in `invariants.md`.

### Merkle storage (`crates/annex-identity/src/merkle.rs`, table `identities`)

- Append-only Poseidon Merkle tree. Leaves are identity commitments stored in the `identities` SQL table (migrations 001+).
- On boot, the tree is rebuilt from `identities` and the recomputed root is compared against a persisted root; mismatch raises `MerkleRootMismatch` (see `merkle.rs` around line 232–238) and panics. This is the only "tamper detection" today.
- Roots are formatted by encoding the Fr field element as big-endian bytes via `into_bigint().to_bytes_be()` then `hex::encode(...)` — lowercase, no `0x`, fixed width.
- The current root is exposed at `GET /api/registry/root` (see `crates/annex-server/src/api.rs::get_current_root_handler`, route registered in `lib.rs` near line 762).
- Path lookup for clients uses `annex_identity::registry::get_path_for_commitment`.

### Nullifier tracking (`crates/annex-identity/src/nullifier.rs`, table `zk_nullifiers`)

- `nullifierHex = sha256(commitmentHex + ":" + topic)` (computed client-side; format defined in `AGENTS.md`).
- DB columns: `(topic, nullifier_hex, pseudonym_id, commitment_hex)`. The latter two are denormalized lookup columns added in migration 024 to make pseudonym → commitment resolution O(1) instead of O(N·M).
- Insertion is the single double-join boundary; uniqueness is enforced at the DB level (constraint violation → `IdentityError::DuplicateNullifier`).

### Current limitations

- **No epoch model.** The Merkle root is implicit and timeless. If the tree is ever rotated (re-key, mass deletion, bulk identity migration), in-flight proofs against the previous root would be silently rejected by the equality check in middleware. There is no negotiation, no notion of "valid in epoch N".
- **Nullifier scoping is per-topic only.** There is no per-epoch or per-server-slug nullifier prefix. Two servers that happen to import the same identity registry would collide on nullifier rows.
- **Root canonical form is implicit.** The middleware compares `current_root` (a hex string from `tree.root_hex()`) against `payload.root_hex` (a hex string from the client). Both happen to use lowercase no-prefix; a client serializing differently would silently fail. There is no normalisation layer.
- **Trusted setup is not auditable.** Single-machine ceremony, ephemeral entropy, no public contribution log. Acceptable for staging; insufficient for a public release that claims production-grade ZK.
- **Vkey provenance is not enforced AT RUNTIME.** Build-time verification now exists (see "Implemented: dev / production artifact split" below) but the server does not yet recompute and assert the vkey hash on startup. A swap between build and install is still undetected by the running process.
- **CI workflow `.github/workflows/release-desktop.yml`** uses `|| true` after the ZK setup step on Windows/macOS to avoid failing the build when the snarkjs ceremony hits transient errors. That fallback can produce a `membership_vkey.json` that is the literal `'{}'` placeholder. A release artifact built from that tree ships an empty vkey. **Mitigation**: set `ANNEX_BUILD_PROFILE=production` in that workflow so `build-desktop.js` calls `verify-artifacts.js`, which refuses an empty / mismatched vkey. This is a one-line follow-up; until it lands, the `|| true` is the remaining hole.

---

## Required artifact manifest

A production release MUST ship the following ZK artifacts, with the
described properties:

| Artifact                            | Path                                  | Property                                                                                                  |
| ----------------------------------- | ------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| Membership circuit (R1CS)           | `zk/build/membership.r1cs`            | Built from `zk/circuits/membership.circom`. Reproducible build (matching circom version pinned).          |
| Membership wasm                     | `zk/build/membership_js/membership.wasm` | Same circom build. Shipped to clients via `client/public/zk/membership.wasm`.                          |
| Membership zkey (proving key)       | `zk/keys/membership_final.zkey`       | Output of a documented multi-party Phase 2 ceremony. Hash recorded in release notes.                      |
| Membership vkey                     | `zk/keys/membership_vkey.json`        | `snarkjs zkey export verificationkey` of the above. Bundled into desktop. Hash MUST match the proving key. |
| Powers of tau (Phase 1)             | `zk/keys/pot14_final.ptau`            | Public ceremony PoT (e.g. Hermez Phase 1) at minimum depth 14. Hash documented.                           |
| Identity circuit + key (optional)   | `zk/build/identity.*`, `zk/keys/identity_*` | Used for client-side commitment derivation; not strictly needed at server boot but shipped for consistency. |

A release manifest file (e.g. `release_v0.X.md`) MUST list each artifact's
SHA-256 alongside the build commit. Verifiers can hash the bundled
`membership_vkey.json` and compare to the manifest.

---

## Required root epoch model

Replace the implicit "the root just is" model with an explicit epoch tag.

**Schema change (new migration, e.g. `034_merkle_epochs.sql`):**

```sql
CREATE TABLE merkle_epochs (
    epoch_id        INTEGER PRIMARY KEY AUTOINCREMENT,
    root_hex        TEXT NOT NULL,
    leaf_count      INTEGER NOT NULL,
    activated_at    INTEGER NOT NULL,        -- unix seconds
    retired_at      INTEGER,                 -- null while active
    server_slug     TEXT NOT NULL,
    notes           TEXT
);
CREATE UNIQUE INDEX idx_merkle_epochs_active
    ON merkle_epochs(server_slug)
    WHERE retired_at IS NULL;
```

**Runtime additions:**

- `crates/annex-identity/src/merkle.rs` exposes `current_epoch_id()` alongside `root_hex()`.
- The proof submission API (`/api/registry/auth/...` + WS) accepts a public-input pair `[root, commitment]` and validates the root against `merkle_epochs` (active OR within a documented grace period after retirement). Proofs against retired epochs are **rejected**, not silently ignored.
- `derive_server_slug_from_public_url` (already present in `config.rs`) must remain stable; the epoch table keys on it.
- Federation: peers exchange their current `(server_slug, epoch_id, root_hex)` during the VRP handshake, so cross-server proof acceptance is unambiguous.

**Why an epoch:** every key rotation, mass-import, or schema change that
touches the leaf set creates a new root. Without an epoch, in-flight clients
either see "your proof is invalid" without context, or worse, server logic
silently accepts an old proof against an old root because the comparison
happened to be on the previous boot.

---

## Required canonical hex model

Define and enforce a single canonical encoding for every field element on
the wire.

**Specification:**

- A field element on the wire is a fixed-width 64-character lowercase hex string with no `0x` prefix and no leading-zero trimming.
- Big-endian byte order: `into_bigint().to_bytes_be()` for `Fr` then `hex::encode`. This is what `crates/annex-identity/src/merkle.rs` already produces; codify it.
- `commitment_hex` and `nullifier_hex` follow the same convention.
- Inputs are normalised at the boundary: `crates/annex-server/src/middleware.rs` MUST lowercase + strip `0x` prefix on inbound `root_hex` / `commitment_hex` / `nullifier_hex` before comparison. Currently the comparison is byte-exact, which fails silently on case differences.

**Test:**

A new unit test in `crates/annex-server/src/middleware.rs` (or a dedicated
`tests/canonical_hex.rs`) must cover:
- 64-char lowercase passes through unchanged.
- 64-char uppercase is normalised to lowercase before comparison.
- `0x`-prefixed input is normalised to no-prefix.
- 63-char (leading-zero stripped) input is rejected with a typed error, not silently rehydrated. (Stripping is a footgun; require fixed width.)
- Non-hex input is rejected with a clear error.

**Spec doc:** copy this section into `docs/protocol/` once the encoding is implemented.

---

## Implemented: dev / production artifact split

A first slice of the production posture is in place. Random-entropy "build-time
trusted setup" is now a **dev-only** path; production builds consume pinned
artifacts whose hashes are verified against a manifest before the bundle
moves forward.

### Files

- `zk/artifacts/<circuit>/manifest.json` — committed manifest. For
  `circuit=membership` it pins the SHA-256 of `wasm`, `zkey`, `vkey`, and
  `r1cs`, plus circuit metadata (`circuit`, `circuitVersion`, `curve`,
  `provingSystem`, `treeDepth`, `publicSignals`). The
  `ceremony.type` field labels how the pinned artifacts were produced.
  Today every shipped manifest carries `ceremony.type: "dev-fixture"`;
  flipping that to `"ceremony-vN"` is the documentation event that
  accompanies a real ceremony.
- `zk/scripts/verify-artifacts.js` — side-effect-free verifier. Reads a
  manifest (default `zk/artifacts/membership/manifest.json`), computes
  SHA-256 of each referenced file, exits 0 only if every required artifact
  is present and matches. Prints a loud warning when
  `ceremony.type == "dev-fixture"`.
- `zk/scripts/dev-setup-groth16.js` — the random-entropy trusted setup that
  used to live in `zk/scripts/setup-groth16.js`. Banners loudly that it is
  dev-only, refuses to run when `ANNEX_BUILD_PROFILE=production` (or
  `release`).
- `zk/scripts/setup-groth16.js` — preserved as a thin compat shim that
  delegates to `dev-setup-groth16.js`. Existing call sites
  (`scripts/claude-setup.sh`, `crates/annex-identity/tests/common.rs`, ad
  hoc CI lanes) keep working without modification, but the dev-only nature
  is now explicit in their output.

### Profiles

`scripts/build-desktop.js` reads `ANNEX_BUILD_PROFILE`:

| Profile      | ZK behaviour                                                  | Missing client artifact |
| ------------ | ------------------------------------------------------------- | ----------------------- |
| `dev` (default) | runs `dev-setup-groth16.js` if artifacts are absent         | warn, continue         |
| `production` (or `release`) | runs `verify-artifacts.js`; never generates new keys; `SKIP_ZK=1` is rejected | hard fail (`process.exit(1)`) |

`scripts/prepare-zk-dev.js` (used by `cargo tauri dev` and the standalone
Vite dev server) also reads `ANNEX_BUILD_PROFILE` and refuses to run in
production. There is no path by which `prepare-zk-dev.js` can land random
keys into a production bundle.

### Files copied into `client/public/zk`

In both profiles, on success, exactly two files are placed under
`client/public/zk/` for the Vite dev server / Vite build to pick up:

| Destination                                       | Source                                          |
| ------------------------------------------------- | ----------------------------------------------- |
| `client/public/zk/membership.wasm`                | `zk/build/membership_js/membership.wasm`        |
| `client/public/zk/membership_final.zkey`          | `zk/keys/membership_final.zkey`                 |

The verification key (`zk/keys/membership_vkey.json`) is **not** placed in
`client/public/zk`; it is consumed only by the server, bundled into the
Tauri app via `crates/annex-desktop/tauri.conf.json::bundle::resources` and
loaded at boot in `crates/annex-server/src/lib.rs`.

### Dev path

```
$ node scripts/build-desktop.js                   # ANNEX_BUILD_PROFILE=dev (implicit)
[build-desktop] profile: dev
[build-desktop] ZK artifacts already exist — skipping ZK build (dev)
[build-desktop] Copying ZK artifacts to client/public/zk/...
[build-desktop]   Copied membership.wasm
[build-desktop]   Copied membership_final.zkey
…
```

If `zk/build/...` and `zk/keys/...` are empty, `build-desktop.js` runs
`build-circuits.js` and then `dev-setup-groth16.js`. The freshly-generated
keys will not match the pinned manifest hashes (different entropy each
run). That's expected and harmless in dev: the manifest is consulted only
in production mode.

### Production path

```
$ ANNEX_BUILD_PROFILE=production node scripts/build-desktop.js
[build-desktop] profile: production
[build-desktop] Verifying pinned ZK artifacts against manifest...
[build-desktop]   $ node scripts/verify-artifacts.js
[verify-artifacts] manifest: …/zk/artifacts/membership/manifest.json
[verify-artifacts] circuit: membership (version 1.0.0)
[verify-artifacts] proving system: groth16 over bn254, tree depth 20
[verify-artifacts] public signals: [root, commitment]
[verify-artifacts] WARN manifest is marked ceremony.type="dev-fixture". …
[verify-artifacts]   OK wasm …/zk/build/membership_js/membership.wasm (sha256 c6d5057059a96961…)
[verify-artifacts]   OK zkey …/zk/keys/membership_final.zkey         (sha256 d8991bf1fc81a335…)
[verify-artifacts]   OK vkey …/zk/keys/membership_vkey.json          (sha256 9412beaaeaab3d68…)
[verify-artifacts]   OK r1cs …/zk/build/membership.r1cs              (sha256 a9a7976e08b8fa52…)
[verify-artifacts] All artifacts verified against manifest.
[build-desktop] ZK artifacts verified.
[build-desktop] Copying ZK artifacts to client/public/zk/...
[build-desktop]   Copied membership.wasm
[build-desktop]   Copied membership_final.zkey
…
```

Production mode never generates new keys. If artifacts are missing or
hash-mismatched, `verify-artifacts.js` exits non-zero, propagating into
`build-desktop.js`'s `execSync`, which terminates the bundle build.

### Replacing the dev-fixture with a real ceremony

When a multi-party ceremony output is ready:

1. Drop the new `membership.wasm`, `membership_final.zkey`,
   `membership_vkey.json`, and `membership.r1cs` into `zk/build/...` and
   `zk/keys/...` at the manifest's `paths.*` locations.
2. Regenerate hashes (e.g. `sha256sum zk/build/membership_js/membership.wasm`)
   and update the four `*_sha256` fields in
   `zk/artifacts/membership/manifest.json`.
3. Set `ceremony.type` to a stable name like `"ceremony-v1"` and fill out
   `ceremony.note` (or add a richer `ceremony.contributions` array — the
   schema allows it).
4. Bump `circuitVersion` if the underlying circuit changed.
5. Run `node zk/scripts/verify-artifacts.js` locally and confirm
   `[verify-artifacts] All artifacts verified against manifest.` plus no
   `dev-fixture` warning.
6. Commit the manifest. Production builds elsewhere will refuse anything
   that doesn't match.

The dev path keeps working — it doesn't read the manifest.

---

## Implemented: membership v2 with secret-derived nullifier

A second slice of the production posture is in place: the membership
circuit now has a v2 variant that derives the per-topic nullifier from
the holder's **secret key** inside the circuit, rather than from the
public commitment outside it. The two versions ship side-by-side; the
server dispatches incoming proofs to exactly one verifier by an
explicit `protocolVersion` field and never silently mixes them.

### Why v2

v1 derives the nullifier as `sha256(commitmentHex + ":" + topic)` on the
client. The commitment is a public Merkle leaf, so anyone with read
access to the registry — federation peers, leaked snapshots, ex-operators
— can compute every pseudonym for every topic. That is not zero-knowledge;
it is a deterministic public mapping from leaf to handle.

v2 binds the nullifier to `sk` inside the circuit:

    nullifier = Poseidon(sk, topicHash, DOMAIN_NULLIFIER_V2)

Knowledge of `sk` is now required to compute the nullifier. The
commitment alone is not enough.

### Files

- `zk/circuits/membership_v2.circom` — the new circuit. Same Merkle
  inclusion proof and `leafIndex ↔ pathIndexBits` binding as v1, plus a
  Poseidon-3 hash of `(sk, topicHash, DOMAIN_NULLIFIER_V2)`. Public
  signals (in snarkjs ordering): `[root, commitment, nullifier, topicHash]`
  — length 4. `topicHash` is declared `public` so the verifier supplies
  it; `DOMAIN_NULLIFIER_V2 = 1` is a hard-coded domain separator.
- `zk/scripts/build-circuits.js` and `zk/scripts/dev-setup-groth16.js`
  — both now build / set up `membership_v2` alongside `identity` and
  `membership`. v1 artifacts are untouched.
- `zk/scripts/test-proofs.js` — six new v2 assertions:
    1. valid v2 proof verifies; `publicSignals[2]` matches
       `Poseidon(sk, topicHash, 1)`; `publicSignals[3]` echoes `topicHash`.
    2. tampering `publicSignals[2]` (the nullifier) is rejected.
    3. tampering `publicSignals[3]` (the topicHash) is rejected.
    4. mismatched `leafIndex` vs `pathIndexBits` rejected at witness
       generation (same invariant as v1).
    5. same `sk` + same `topicHash` produces the same nullifier
       (deterministic — required for double-join detection).
    6. same `sk` + different `topicHash` produces a different nullifier
       (required so per-topic pseudonyms are unlinkable).

### Server config + dispatch

- `Config::security.enabled_zk_versions: Vec<String>` (default `["v1"]`).
  Recognised values: `"v1"`, `"v2"`. Anything else fails startup with
  `StartupError::UnknownZkVersion`.
- `AppState::membership_vkey_v2: Option<Arc<VerifyingKey<Bn254>>>`.
  Loaded only when `"v2"` is enabled. Path priority:
  `ANNEX_ZK_KEY_PATH_V2`, otherwise `zk/keys/membership_v2_vkey.json`.
  Same enforcement as v1: with `enforce_zk_proofs = true`, missing or
  invalid v2 vkey is `StartupError::MissingVerificationKey`.
- `VerifyMembershipRequest` adds three optional fields:
    - `protocolVersion: Option<String>` — `None` or `"v1"` selects the
      legacy verifier; `"v2"` selects the secret-derived-nullifier
      verifier and requires the v2 vkey to be loaded.
    - `nullifierHex: Option<String>` — v2 only. Cross-checked against
      `publicSignals[2]` after canonical-hex normalisation, so a
      malicious response cannot substitute a different nullifier than
      the one the prover actually computed inside the circuit.
    - `topicHashHex: Option<String>` — v2 only. Cross-checked against
      `publicSignals[3]`.
- `verify_membership_handler` (`crates/annex-server/src/api.rs`):
    1. Resolves `protocolVersion` first, before any DB or proof work,
       so an unknown version is `400 Bad Request` regardless of state.
    2. Selects the right vkey + expected `publicSignals.len()` (2 for
       v1, 4 for v2).
    3. Verifies the proof against the version-matched vkey. A v2 proof
       against the v1 vkey (or vice-versa) is rejected as a
       verification failure — the vkey size encodes the public-input
       count.
    4. Validates `publicSignals[0]` ↔ claimed root and `publicSignals[1]`
       ↔ claimed commitment in both versions.
    5. v2 only: extracts `nullifier_v2_hex = canonical(publicSignals[2])`
       and uses **that** as the nullifier going into double-join
       tracking and pseudonym derivation. v1's
       `derive_nullifier_hex(commitment, topic)` is never called for
       a v2 proof.
- `crates/annex-server/tests/zk_startup.rs` adds four v2-specific tests:
  default `enabled_zk_versions == ["v1"]`; unknown version is a
  startup error; v2 enabled with v2 vkey present boots cleanly; v2
  enabled with v2 vkey missing under enforcement is
  `StartupError::MissingVerificationKey`.
- `crates/annex-server/tests/api_zk_v2.rs` covers routing: v2 payload
  on a v1-only server → `409 Conflict`; unknown `protocolVersion`
  → `400 Bad Request`; missing `protocolVersion` → defaults to v1
  (does not return the v2-not-enabled message).

### Migration semantics — keep v1 intact

- v1 keys, vkey, circuit, and on-the-wire shape are **untouched**. The
  only behavioural change for v1 clients on v1-only servers is: none.
- v2 is opt-in per server (`enabled_zk_versions` must include `"v2"`)
  AND opt-in per request (`protocolVersion: "v2"`). A server that
  enables both still rejects v1 payloads against the v2 vkey and
  vice versa.
- v1 nullifiers in `zk_nullifiers` and v2 nullifiers in the same table
  are different 64-char hex strings; rows do not collide.
- v1 cannot be removed until every client has been updated. Both
  versions remain enabled during the migration window.

### Open follow-ups

These are deliberately out of scope for the v2-introduction task and
are tracked here:

- **Server-side `topic → topicHash` mapping**. Today the server accepts
  the prover-supplied `topicHashHex` and binds the proof to it. That is
  enough to detect double-joins per-topic (because same sk+topicHash
  always produces the same nullifier), but it lets a malicious client
  claim a proof was made for topic A while binding `topicHash` for
  topic B. Closing this requires a Poseidon-of-string convention
  shared by client and server. Until it lands, the server records
  both `payload.topic` and `publicSignals[3]` in the audit log so the
  discrepancy is observable.
- **v1 retirement**. Once every shipped client has switched to v2 and
  every active VRP nullifier is v2-derived, drop v1 from
  `enabled_zk_versions`, then remove the v1 wasm/zkey bundle and the
  v1 verification path. No active deployment is at this stage yet.
- **Federation `protocolVersion` exchange**. Two federated servers
  must both be on v2 (or both on v1) for cross-server proof acceptance
  to work. The handshake envelope in `crates/annex-federation::handshake`
  needs to advertise the supported set and reject mismatched peers.
- **Client-side v2 prover**. `client/src/lib/zk.ts` and the proof
  worker still build v1 proofs only. A future task adds the v2 prover
  with `topicHash` as a public input and `protocolVersion: "v2"` in
  the verify-membership request.

---

## Migration sequence to reach the target

Each step is its own task using the template in `agent-playbook.md`. Steps
are ordered so each one stands on its own and can land independently.

1. **Canonical hex normalisation.** Add the boundary normaliser in `middleware.rs`; cover with unit tests. Touch only `crates/annex-server/src/middleware.rs` and a new test file. No DB change.

2. **Vkey hash assertion at boot.** Compute and log the SHA-256 of the loaded vkey in `crates/annex-server/src/lib.rs`. Add a new optional `[zk] expected_vkey_sha256` field in `Config` (with a new `default(...)` returning empty), and panic on mismatch when set. Touch `config.rs`, `lib.rs`, plus tests. No DB change.

3. **Epoch table + root activation.** New migration `034_merkle_epochs.sql`. Server boot inserts an epoch row if none exists for the current `server_slug`. Read the active row's root and compare instead of the implicit current root. Touch `annex-db` migrations, `annex-identity::merkle`, `annex-server::api`, `annex-server::middleware`, plus tests. **First task that requires `I-MERKLE-1` epoch awareness to land.**

4. **Federation epoch exchange.** Add `(server_slug, epoch_id, root_hex)` to the VRP handshake envelope (`annex-federation::handshake` + matching client). Reject proofs from peers in epochs we have not negotiated. Touch `annex-federation`, `annex-server::api_federation`. Coordinated wire change — increment the federation envelope version.

5. **Nullifier scoping with `(server_slug, epoch_id)`.** New migration adds a `server_slug` and `epoch_id` to `zk_nullifiers`. Update the insert path in `annex-identity::nullifier`. Backfill existing rows with the current active epoch.

6. **Multi-party trusted setup.** Replace the single-machine `setup-groth16.js` with a documented contribution flow: each contributor publishes their `*_<n>.zkey` + entropy hash. The release notes record the contribution chain. Update `release-desktop.yml` to reject the empty-vkey fallback (`|| true` removal). The release artifact's vkey hash must match the manifest.

7. **Wider PoT.** Once any circuit grows past ~16k constraints, regenerate `pot14_final.ptau` against a depth-15 (or higher) PoT — Hermez or equivalent. Update `setup-groth16.js`.

Each task individually:
- May NOT remove `generate_dummy_vkey()` from `annex-identity::zk` — it is still the dev-only fallback.
- May NOT change the `[root, commitment]` public-input order — that's locked by `I-ZK-3`.
- MUST NOT edit a published migration — always add a new one (`I-DB-1`).
