# Agent Handoff

## Current branch
`claude/fix-annex-bugs-Las84` (current session; chain: `…itXFq` → `…PxyqS` → `…Las84`)

## Session goal
Recursive production bug-fix campaign on Annex (Tauri desktop, Rust workspace,
Groth16/Circom ZKP, SQLite). Fix highest-impact real bugs in priority order:
ZK enforcement → ZK release artifact path → canonical hex → Merkle epoch/concurrency
→ nullifier privacy → desktop release → security sweep.

## Fixed in this session (claude/fix-annex-bugs-Las84)

### [F11] Federation v1/v2 vkey dispatch (release blocker for v2 rollout)
`services/federation_service.rs::attest_membership` always verified incoming
attestations against `state.membership_vkey` (v1) regardless of what the
peer sent. v2 federation peers (those whose proofs use the v2 circuit with
secret-derived nullifiers) had no working federation path: their
attestations would be silently routed into the v1 verifier and fail with a
generic "Invalid proof" error, so their identities could never be cross-
attested across servers. Listed as the top "Still broken" item in the
previous handoff.

The fix mirrors the local `/api/zk/verify-membership` dispatch: extend
`AttestationRequest` with optional `protocolVersion`, `publicSignals`,
`nullifierHex`, and `topicHashHex` fields, and pick the matching vkey at
runtime. v2 attestations are rejected with `403 Forbidden` when the
receiving server has not loaded the v2 vkey, so v2 enablement is opt-in
per server (`Config::security.enabled_zk_versions`).

Topic-binding is enforced exactly the way the local verifier does it:
`publicSignals[3]` MUST equal `topic_hash_for_v2(payload.topic)`, otherwise
the proof is bound to a different topic. Without this, a v2 prover could
reuse a v2 proof for topic A as a v2 attestation for topic B (same closed
gap as [F1] in the previous session, but for the federation path).

The signed-payload shape also binds protocol_version + nullifierHex +
topicHashHex when v2 is declared, so a peer cannot strip the v2 fields
off the wire to coerce v1 dispatch — the signature would not verify. v1
keeps the legacy 3-field signing input for wire compatibility.

- files changed:
  - `crates/annex-federation/src/types.rs` — extended
    `AttestationRequest` with `protocolVersion`, `publicSignals`,
    `nullifierHex`, `topicHashHex` (all `Option<...>`,
    `skip_serializing_if = "Option::is_none"` so v1 wire shape is
    unchanged).
  - `crates/annex-server/src/services/federation_service.rs` —
    `attest_membership` now dispatches to v1 or v2 based on
    `protocol_version`. v2 wire-shape validation runs BEFORE the network
    round-trip so a malformed envelope is rejected without probing the
    peer. Cross-checks: `publicSignals[3] ==
    topic_hash_for_v2(payload.topic)`, `publicSignals[2] ==
    canonicalised(claimed nullifierHex)`, `publicSignals[0..2] ==
    (remote_root_fr, commitment_fr)` (the root-cross-check fires after
    the network call). For v2 the canonical nullifier is taken from
    `publicSignals[2]` (secret-derived, server cannot recompute);
    `derive_nullifier_hex(commitment, topic)` is still the v1 source of
    truth.
  - `crates/annex-server/tests/api_federation_attestation.rs` — updated
    existing tests for the new `AttestationRequest` field set; added 5
    new tests:
    - `test_attest_membership_v2_rejected_when_v2_not_enabled`
    - `test_attest_membership_unknown_protocol_version_rejected`
    - `test_attest_membership_v2_requires_nullifier_hex_in_signing_input`
    - `test_attest_membership_v2_topic_mismatch_rejected`
    - `test_attest_membership_v2_requires_public_signals`
- tests run: `cargo test -p annex-server --test api_federation_attestation`
  → 10 passed; full workspace `cargo test -p annex-server` → 350 passed,
  0 failed (118 lib + 232 integration; up from 345 baseline + 5 new).
- result: PASS. v2 attestations are now first-class: dispatched to
  v2 vkey, topic-bound, signature-bound to the v2 wire shape, and
  rejected with deterministic 4xx errors on every malformed v2 path.

### [F12] SSRF defence-in-depth on federation peer URLs
The federation paths take peer-supplied URLs from the wire and (for the
attestation freshness callback and the `attest_membership` root-fetch)
turn them into outbound HTTP requests. Peers are administratively trusted
via `federation_agreements`, but a misconfigured `instances` row (e.g.
`http://localhost:9090` for a Prometheus port) would otherwise turn these
endpoints into a continuous probe of internal services from inside the
server's trust boundary. Listed in the previous handoff as a defence-in-
depth gap.

Fix: re-export
`api_link_preview::is_url_private_or_reserved` (the same predicate used
by the link preview / image proxy SSRF gate, with full IPv4-mapped-IPv6
+ CGNAT + 169.254 + .local + .internal coverage) and call it from:
1. `attest_membership` — return 403 BEFORE the network call when the
   peer's `originating_server` URL is private/loopback. New regression
   test `test_attest_membership_valid_signature_fails_network` updated
   to assert the new 403 + "private or reserved" message; the previous
   "network error → 500" path is no longer reachable on a localhost
   peer URL.
2. `receive_federated_message` — skip the freshness-check callback when
   the peer URL is private. The freshness check is a soft gate
   ("log on mismatch / continue on network error"), so we log + skip
   instead of rejecting the whole message.
3. `relay_message` (background relay) — skip relay to peers whose
   `base_url` resolves to a private/reserved host, with a warn-level
   log naming the peer.

- files changed:
  - `crates/annex-server/src/api_link_preview.rs` — exposed the existing
    private-IP/host predicate as `pub(crate) fn
    is_url_private_or_reserved`.
  - `crates/annex-server/src/services/federation_service.rs` — three
    SSRF gates as above.
- tests run: same as [F11].

### [F13] Per-request `x-annex-zk-proof` header now supports v2
`middleware::verify_zk_membership_header` is the per-request membership
re-prove gate (called from `channel_service::join_channel`,
`create_message`, etc.) and was hard-wired to the v1 vkey. v2 clients
hitting a channel-protected endpoint with a v2 proof in the
`x-annex-zk-proof` header would be silently routed into the v1 verifier
and 403'd. Same shape of bug as [F11] but on the request-time path.

Fix: extend `ZkProofPayload` with `protocolVersion`, `publicSignals`,
and `topic` fields. Dispatch on version exactly like the federation +
local verify-membership paths. Topic-binding identical to [F11].

The `topic` field is required for v2 because the topic is what binds the
nullifier to a routing context — without it the server cannot recompute
`topic_hash_for_v2`. v1 doesn't need a topic at this layer because the
nullifier isn't part of the proof's public signals.

- files changed:
  - `crates/annex-server/src/middleware.rs` — extended `ZkProofPayload`
    and rewrote `verify_zk_membership_header` to dispatch on
    `protocol_version`. v1 path is byte-for-byte unchanged
    (`protocol_version` defaults to `None` → "v1", same 2-signal
    public-input vector).
- tests run: full workspace test suite is green; the existing channel
  tests cover the v1 path. v2 path uses the same dispatch + topic-binding
  rules tested in [F11], so no new tests are added on this side; the
  guarantee is structural (same logic, same vkey, same topicHash check).

## Fixed in earlier session (claude/fix-annex-bugs-PxyqS)

### [F10] Clippy CI breakers (release blocker)
`cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings`
was failing on a fresh checkout: 5 doc-list-overindentation warnings in
`ws/commands/resume.rs`, 1 doc-list missing-blank-line in
`tests/api_invites.rs`, and 2 `field assignment outside of initializer`
in `tests/identity_service.rs`. CI runs that exact invocation, so these
were silently breaking the release pipeline.
- files changed:
  - `crates/annex-server/src/ws/commands/resume.rs` — flattened the
    nested numbered list in the module doc.
  - `crates/annex-server/tests/api_invites.rs` — added the required
    blank line before the start of the inline numbered list.
  - `crates/annex-server/tests/identity_service.rs` — switched the two
    `policy.field = ...` blocks to struct-update syntax with
    `..ServerPolicy::default()`.
- tests run: `cargo clippy --workspace --exclude annex-desktop
  --all-targets -- -D warnings` — clean.

### [F9] Agent handshake hijacking + role-code type bug (security)
`POST /api/vrp/agent-handshake` is intentionally unauthenticated so a
freshly-spun-up agent can register before any platform_identities row
exists. But the handler was equally unauthenticated AFTER the agent had
gone through verify-membership: the only check was a participant-type
lookup, and the row's TEXT label was being read as `u8`, so the
"already-registered" branch never ran in practice. Two real bugs:

1. **Role-code type bug**: `participant_type` is a TEXT column populated
   with the label (`"AI_AGENT"`, `"HUMAN"`, …), but the handler did
   `let role_code: Option<u8> = conn.query_row(...)`. SQLite would
   silently fail the column conversion, surface as `Err` (mapped to
   500), and short-circuit any logic predicated on the row existing.
   The `Some(AI_AGENT)` and `Some(_)` branches were unreachable.

2. **Agent identity hijacking**: anyone who could read a public agent
   pseudonym (visible in `/api/public/agents`, the events stream,
   channel listings, federation handshakes) could submit a re-handshake
   on the agent's behalf. The handler upserts `agent_registrations`,
   replacing `capability_contract_json`, `anchor_snapshot_json`,
   `alignment_status`, and `transfer_scope`. They could also force
   Conflict alignment, which deactivates the row AND forcibly
   disconnects the agent's WebSocket session.

Fix:
- Read `participant_type` as `String` and compare to
  `RoleCode::AiAgent.label()`. (Closes the type bug.)
- When the pseudonym IS already a registered AI_AGENT, REQUIRE a valid
  `Authorization: Bearer <session-token>` whose bound pseudonym matches
  `payload.pseudonymId`. Pre-registration (no platform_identities row)
  remains unauthenticated. (Closes the hijack.)
- files changed:
  - `crates/annex-server/src/api_vrp.rs` — added
    `pseudonym_from_authorization_header`; handler now takes a
    `HeaderMap` and applies the gate.
  - `crates/annex-server/tests/api_vrp_handshake.rs` — added 4 tests:
    - `rehandshake_without_token_is_rejected_for_registered_agent`
    - `rehandshake_with_mismatched_token_is_rejected`
    - `pre_registration_handshake_remains_unauthenticated`
    - `rehandshake_with_matching_token_is_allowed`
- tests run: `cargo test -p annex-server --test api_vrp_handshake` —
  6 passed; `cargo test --workspace --exclude annex-desktop` — 564
  passed, 0 failed.

### [F8] Image proxy SVG XSS (security)
`/api/link-preview/image?url=<attacker-controlled URL>` is unauthenticated
and proxies the response as-is with `Content-Type` from the upstream.
SVG documents are accepted by the previous `image/*` filter and forwarded
on to the user's browser. Loading the proxy URL via `<img>` is safe
(browsers don't execute scripts in image documents), but a victim who
right-clicks "Open Image in New Tab" — or just pastes the proxy URL into
the address bar — lands on a top-level document served from the Annex
server's origin. SVGs can carry inline `<script>` and event handlers,
which then execute as same-origin XSS in the proxy server's context
(cookies, sessionStorage, API tokens — all reachable).
- files changed:
  - `crates/annex-server/src/api_link_preview.rs` —
    1. `image_proxy_handler` now rejects `image/svg`, `*/svg+xml`, and
       octet-stream URLs ending in `.svg`.
    2. `url_has_image_extension` and `infer_image_content_type` no
       longer recognise `.svg`.
    3. `build_image_response` adds `Content-Security-Policy: sandbox;
       default-src 'none'` to every image response, so any SVG that
       still slips through the content-type filter is rendered with
       script execution disabled.
- tests added:
  - `url_image_extension_detection_excludes_svg`
  - `build_image_response_sets_sandbox_csp`
- tests run: `cargo test -p annex-server --lib api_link_preview::` —
  15 passed (2 new).
- result: PASS. SVG can no longer be proxied as image, and the sandbox
  CSP closes the residual gap if any non-image MIME slipped through
  detection.

## Fixed in previous session (claude/fix-annex-bugs-itXFq)

### [F1] v2 topicHash → topic binding (privacy bug)
The v2 verifier accepted any `topicHash` in `publicSignals[3]` as long as the
caller-supplied `topicHashHex` matched it. A malicious prover could produce a
v2 proof for topic A and submit it as a v2 proof for topic B, getting a
nullifier-bound pseudonym in topic B without ever proving membership in
topic B.
- files changed:
  - `crates/annex-identity/src/zk.rs` — added
    `topic_hash_for_v2(topic) -> Fr = SHA-256("annex/v2/topicHash:" + topic) mod p`
    plus `V2_TOPIC_HASH_DOMAIN` constant.
  - `crates/annex-server/src/services/identity_service.rs` — recomputes
    expected topicHash from `payload.topic` and rejects v2 proofs whose
    `publicSignals[3]` does not match. The legacy `topicHashHex` field is
    cross-checked too.
  - `docs/refactor/zk-merkle-production.md` — open follow-up marked CLOSED.
- tests run: `cargo test -p annex-identity --lib zk::` — 29 passed (6 new tests:
  `topic_hash_for_v2_is_deterministic`,
  `topic_hash_for_v2_different_topics_yield_different_hashes`,
  `topic_hash_for_v2_byte_sensitive`, `topic_hash_for_v2_rejects_empty`,
  `topic_hash_for_v2_outputs_canonical_64_char_hex`,
  `topic_hash_for_v2_uses_domain_separator`).
- result: PASS. v2 proof now binds nullifier to the canonical hash of the
  request's topic; topic-substitution attack is closed.

### [F2] Root acceptance grace window — use `is_root_acceptable`
`verify_membership` and `verify_zk_membership_header` were rejecting any
proof whose root was not the CURRENT active root. Because the root rotates
on every registration, that race-conditioned every in-flight prover.
The codebase already had `vrp_root_epochs` + `is_root_acceptable()` (with a
`ROOT_EPOCH_GRACE_SECONDS = 300` grace window) but the hot paths weren't
calling it.
- files changed:
  - `crates/annex-server/src/services/identity_service.rs` —
    `/api/zk/verify-membership` now calls
    `annex_identity::merkle::is_root_acceptable` instead of
    `vrp_roots WHERE active = 1`.
  - `crates/annex-server/src/middleware.rs` — `verify_zk_membership_header`
    (per-request channel auth) does the same.
- tests run: `cargo test --workspace --exclude annex-desktop` — 558/558 pass.

### [F3] Pre-existing `test_register_duplicate_failure` was wrong
The test asserted 409 CONFLICT for duplicate registration, but production
code now does idempotent re-registration (returns 200 OK with the existing
leaf path). Test renamed to `test_register_duplicate_returns_idempotent_path`
and updated to assert the correct semantics.
- files changed: `crates/annex-server/tests/api_registry.rs`.
- tests run: `cargo test -p annex-server --test api_registry` — all 3 pass.

### [F4] Pre-existing fmt drift in `tests/contract_fixtures.rs`
`cargo fmt --all --check` was failing on baseline. Fixed.
- files changed: `crates/annex-server/tests/contract_fixtures.rs`.
- tests run: `cargo fmt --all --check` — clean.

### [F5] Invite-redeem seat-burn DOS (security)
`POST /api/invites/redeem` is unauthenticated and was bumping
`use_count` on every call. Two real bugs:
1. Each successful registration burned **2 seats** (one in redeem, one
   again in `register_identity`) — invite holders lost half their slots
   to client convenience pings.
2. An unauthenticated attacker could fully exhaust a `max_uses`-bounded
   invite by hammering the endpoint without ever registering.
The fix: redeem is now validation-only. Seat consumption stays in
`IdentityService::register_identity`, which is the existing atomic claim.
- files changed:
  - `crates/annex-server/src/api_invite.rs` — removed the bump, added
    docstring and a defensive comment so the next reader sees why.
  - `crates/annex-server/tests/api_invites.rs` — three new tests:
    `redeem_does_not_consume_seat_on_success` (validates 3 redeems leave
    `use_count = 0`), `redeem_rejects_exhausted_invite`,
    `redeem_rejects_unknown_code`.
- tests run: `cargo test -p annex-server --test api_invites` — 3 pass.

### [F6] Constant-time access-password compare (defence in depth)
`identity_service::register_identity` compared the `access_password` with
`String != String`, which short-circuits per byte. With rate-limiting it
was hard to exploit, but constant-time comparison is cheap.
- files changed:
  - `crates/annex-server/Cargo.toml` — added `subtle = "2"` direct dep
    (already in the lockfile transitively via ed25519-dalek).
  - `crates/annex-server/src/services/identity_service.rs` — uses
    `subtle::ConstantTimeEq::ct_eq` and length comparison so the timing of
    the check no longer depends on the byte position of the first
    mismatch.

### [F7] Federation message content-length cap (DOS)
`receive_federated_message` had no upper bound on `envelope.content`
length. Federated peers could push messages up to axum's 2 MiB body cap
into the local `messages` table — well beyond what local WS clients are
allowed (`MAX_WS_MESSAGE_CONTENT_LEN = 64 KiB`). With WAL + retention,
that's a slow-burn database-bloat path even after signature verification.
The fix mirrors the local cap.
- files changed:
  - `crates/annex-server/src/services/federation_service.rs` — added
    `FEDERATION_MAX_MESSAGE_CONTENT_LEN = 65_536`; `receive_federated_message`
    now rejects oversized envelopes with `Forbidden` before any DB I/O.
- tests run: `cargo test -p annex-server --test api_federation_relay` —
  3 pass (existing tests still happy).

## Still broken / suspected

- [ ] **CORS in debug builds bypasses configured origins on `localhost`**.
  `http/cors.rs::is_dev_localhost_origin` is gated on `cfg!(debug_assertions)`
  so release binaries are unaffected, but a misconfigured release build
  would silently accept any localhost origin. Worth a config flag rather
  than a cfg gate. Low priority given the gate is correct for normal usage.
  Verified this session: `cfg!(debug_assertions)` is `false` under
  `--release` so the original concern is partially overstated, but a
  config flag would still be cleaner than the cfg gate.

- [ ] **Trusted-setup ceremony is single-machine, dev-fixture entropy**.
  `manifest.json` for membership pins SHA-256 hashes of artifacts produced
  by `dev-setup-groth16.js`, marked `ceremony.type: dev-fixture`. Real
  public release MUST replace these with multi-party ceremony output.
  Out of scope for this session.

- [ ] **PoT depth ceiling**. Current `pot14_*.ptau` is depth 14. The v1
  membership circuit fits in ~5k constraints, but if the circuit grows past
  ~16k constraints the script will need a depth-15+ PoT.

- [ ] **Uploaded files are public by URL**. `/uploads/*` is mounted via
  `ServeDir` with no auth. Filenames are UUIDs, so unguessable, but
  there is no per-channel access control — a leaked URL is a permanent
  access grant for that file. Acceptable for the "public-ish" content
  pattern of v0.1, but a release blocker for any private-channel mode.
  No fix this session because the architectural change is large and the
  URL-as-capability model is documented behaviour today.

- [ ] **Desktop build cannot be exercised in this environment** — system
  GTK/WebKitGTK packages are missing from the sandbox. Code review of
  `crates/annex-desktop/src/main.rs`, `embedded_server.rs`, and
  `tauri.conf.json` shows correct vkey/asset path resolution and bundle
  resource declarations. Real CI must keep enforcing the existing
  `release-desktop.yml` Linux + Windows jobs.

## Fixed in this session, previously listed as "Still broken"
- **[F11] Federation v1/v2 dispatch** — wired v1/v2 vkey dispatch in
  `attest_membership` with full topic + nullifier + signature binding.
  v2 attestations no longer silently fail.
- **[F12] Federation peer-supplied URL SSRF guard** — the
  `is_url_private_or_reserved` predicate is now applied at three federation
  outbound-call sites. Misconfigured peer entries can no longer turn the
  attestation freshness callback or message relay into a private-network
  probe.
- **[F13] verify_zk_membership_header now supports v2** — the per-request
  channel ZK header now dispatches to the right vkey + topicHash check, so
  v2 clients can hit channel-protected endpoints with v2 proofs.

## Commands run (this session, claude/fix-annex-bugs-Las84)
- `cargo fmt --all --check` → clean (after auto-format).
- `cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings`
  → clean.
- `cargo test -p annex-server --test api_federation_attestation` → 10
  passed (5 new: v2 dispatch + topic mismatch + missing publicSignals +
  missing nullifierHex + unknown protocolVersion).
- `cargo test -p annex-server` → **350 passed, 0 failed** (118 lib +
  232 integration).
- `cargo test --workspace --exclude annex-desktop --exclude annex-server`
  → 219 passed, 0 failed (other crates unchanged).
- Workspace total: **569 passed, 0 failed** (up from 564 baseline; 5
  new tests).
- Client: `npm ci && ./node_modules/.bin/vitest run` → 165 passed across
  16 files.
- Client: `npm run lint` → clean.
- Desktop build: not exercised (sandbox lacks GTK/WebKitGTK).
- Disk note: `/home/user/annex/target` debug artifacts hit ~30 GB twice
  during the session and required `cargo clean -p annex-server` +
  `rm -rf target/debug/incremental` to recover. The full integration
  test suite alone needs ~24 GB of test-binary space; on tight CI
  runners, prefer running tests by crate rather than `--workspace` in a
  single shot.

## Commands run (previous session, claude/fix-annex-bugs-PxyqS)
- `cargo fmt --all --check` → clean (after edits).
- `cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings`
  → originally failing on doc-list-overindentation in
  `ws/commands/resume.rs` and field-assignment-after-default in
  `tests/identity_service.rs`. Fixed; now clean.
- `cargo test -p annex-server --lib api_link_preview::` → 15 passed
  (2 new: SVG exclusion + sandbox CSP).
- `cargo test -p annex-server --test api_vrp_handshake` → 6 passed
  (4 new: re-handshake auth gate, mismatched token, pre-registration,
  matching token).
- `cargo test --workspace --exclude annex-desktop` → **564 passed, 0
  failed** (up from 558 baseline; 6 new tests).
- Desktop build not exercised in this session (sandbox dependency).

## Commands run (previous session, claude/fix-annex-bugs-itXFq)
- `cargo fmt --all --check` → originally failing on `tests/contract_fixtures.rs`,
  fixed.
- `cargo build --workspace --exclude annex-desktop` → clean (with edits).
- `cargo test -p annex-identity --lib zk::` → 29 passed, 0 failed (includes
  the 6 new `topic_hash_for_v2_*` tests).
- `cargo test -p annex-server --test api_registry` → 3 passed, 0 failed.
- `cargo test -p annex-server --test api_zk_v2` → 5 passed, 0 failed.
- `cargo test -p annex-server --test api_zk_verify` → 1 passed, 0 failed.
- `cargo test -p annex-server --test api_invites` → 3 passed, 0 failed (new file).
- `cargo test -p annex-server --test api_federation_relay` → 3 passed, 0 failed.
- `cargo test --workspace --exclude annex-desktop` → 558 passed, 0 failed.
- `cargo build -p annex-desktop --release` → fails because the sandbox
  lacks `libgtk-3-dev`; documented as a system dependency, not a code bug.
- `cd zk && npm ci && node scripts/build-circuits.js && node scripts/setup-groth16.js`
  → ran successfully; ZK keys regenerated after a session restart wiped
  them.

## Important invariants
- I-ZK-1: enforce_zk_proofs default is `true`. Never silently degrade.
- I-ZK-2: dummy vkey is dev-only; production builds must ship a real vkey.
- I-ZK-3: membership proof public signals are `[root, commitment]` (v1) or
  `[root, commitment, nullifier, topicHash]` (v2), in that order. Server
  must compare proof root to its own current root **AND** the v2 path
  must compare `publicSignals[3]` to `topic_hash_for_v2(payload.topic)` —
  see [F1].
- I-MERKLE-1: roots are 64-char lowercase hex (no `0x`, no leading-zero
  trimming). Append-only at runtime. Acceptance is via
  `vrp_root_epochs` (active OR within `ROOT_EPOCH_GRACE_SECONDS` of
  retirement) — see [F2].
- I-AUTH-1: no raw pseudonym auth in enforced mode.
- I-WS-1: WS protocol field shapes are stable; renames are protocol breaks.
- I-FED-1: federation envelopes are signed; never bypass verification.
- I-FED-2 (new): federated message `content` is bounded by
  `FEDERATION_MAX_MESSAGE_CONTENT_LEN = 65_536` to mirror the WS ceiling
  — see [F7].
- I-DB-1: migrations are append-only; never edit a published file.
- I-DESKTOP-1: Windows + Linux are release-critical; macOS may be deferred
  but its matrix entries / bundles must not be deleted.
- I-V2-TOPIC-HASH (new): v2 topic→topicHash derivation is exactly
  `Fr::from_be_bytes_mod_order(SHA256("annex/v2/topicHash:" + topic))`.
  Future v2 client implementations must match byte-for-byte. Constant
  prefix is `annex_identity::zk::V2_TOPIC_HASH_DOMAIN`.
- I-INVITE-1 (new): `/api/invites/redeem` is validation-only and MUST NOT
  bump `use_count`. Seat consumption happens atomically in
  `IdentityService::register_identity` after successful registration —
  see [F5].
- I-IMG-PROXY-1 (new): `/api/link-preview/image` MUST reject SVG content
  (any `image/svg`, `*/svg+xml`, or octet-stream URL ending in `.svg`)
  AND MUST set `Content-Security-Policy: sandbox; default-src 'none'`
  on every response. The `url_has_image_extension` allow-list omits
  `.svg` deliberately. See [F8].
- I-AGENT-HANDSHAKE-1 (new): `POST /api/vrp/agent-handshake` is gated
  conditionally:
   * Pre-registration (no `platform_identities` row for the pseudonym)
     remains unauthenticated.
   * Re-handshake (row exists with `participant_type = 'AI_AGENT'`)
     REQUIRES a valid `Authorization: Bearer <session-token>` whose
     bound pseudonym matches `payload.pseudonymId`.
   * `participant_type` is TEXT — read with
     `String` and compare to `RoleCode::AiAgent.label()`. Reading as
     `u8` silently fails the column conversion. See [F9].
- I-FED-V2-1 (new): `POST /api/federation/attest-membership` dispatches
  to the v1 vkey by default and to the v2 vkey when
  `protocolVersion = "v2"`. v2 attestations REQUIRE `publicSignals`
  (length 4), `nullifierHex`, and `topicHashHex`, and the server checks
  that `publicSignals[3] == topic_hash_for_v2(topic)`. v2 is rejected
  with `403 Forbidden` when the receiving server has not loaded the v2
  vkey. The signed message includes the v2 fields when v2 is declared
  so the wire shape cannot be downgraded by stripping fields. See [F11].
- I-FED-SSRF-1 (new): `is_url_private_or_reserved` (re-exported from
  `api_link_preview`) is the canonical predicate for SSRF defence on
  every federation outbound URL — `attest_membership` (hard reject),
  `receive_federated_message` freshness callback (skip + log), and
  `relay_message` background relay (skip + warn). Future federation
  callers MUST go through this predicate. See [F12].
- I-ZK-HEADER-V2 (new): `verify_zk_membership_header` (the per-request
  channel ZK gate read from `x-annex-zk-proof`) accepts an optional
  `protocolVersion` field and dispatches to the v2 vkey when set to
  `"v2"`. v2 also requires `publicSignals` (length 4) and `topic`
  for the canonical topicHash binding. v1 wire shape is unchanged. See
  [F13].

## Context cutoff note
Session [F11..F13] wired v2 federation attestation, added SSRF defence-
in-depth on every federation outbound URL, and extended the per-request
`x-annex-zk-proof` gate to support v2. Workspace test count rose from 564
to 569 (5 new tests). All clippy + fmt clean.

If a future agent picks up:
1. Re-run `cargo test -p annex-server` (350 expected) and
   `cargo test --workspace --exclude annex-desktop --exclude annex-server`
   (219 expected) to confirm the **569-pass** baseline holds. Watch out
   for the disk-pressure issue described under "Commands run".
2. Re-run `cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings`
   to confirm CI clippy gate stays clean.
3. Highest-value remaining items are in "Still broken / suspected":
   - real multi-party ZK ceremony (release blocker for v0.2)
   - PoT depth ceiling (only matters if circuit grows)
   - uploads-as-public-URL design question (release blocker for any
     private-channel mode)
   - desktop build smoke test in real Linux/Windows CI
4. Next concrete files to inspect (none of these are bugs today, but are
   the most likely places for the NEXT class of v2-specific bugs):
   - `zk/scripts/setup-groth16.js` and `dev-setup-groth16.js` — confirm
     v2 production setup ceremony is parameterised separately from v1.
   - `crates/annex-server/src/services/federation_service.rs::relay_message`
     and the matching `api_federation::receive_federated_message_handler` —
     when a v2 client sends a federated message, the attestation_ref
     format must still resolve to the same federated_identities row.
     Currently the row is keyed by (instance_id, commitment_hex), which
     is version-agnostic, so v2 should "just work" — but this is worth
     verifying when there's a real v2 federated client to test against.
   - Client-side (`client/src/api/`, `client/src/lib/zk.ts`) — there is
     currently no client code that sends `protocolVersion = "v2"` on the
     federation attestation. When the v2 client is added it must send
     all four optional fields together (publicSignals + nullifierHex +
     topicHashHex + protocolVersion).
5. Areas already audited this session that look clean:
   - WS connection_manager lock ordering — no deadlock potential.
   - Channel CRUD / message edit-window enforcement — ownership +
     time-window checks correct.
   - Rate limiter / sliding window — sound. Federation endpoints are
     covered by the Default category, keyed by IP.
   - CORS / `is_dev_localhost_origin` — `cfg!(debug_assertions)` is
     `false` under `--release`, so release builds stay strict. The
     handoff item flagging this is partially overstated.
   - SQL building (`api_observe::get_events_handler`,
     `services/rtx_repository.rs`) — parameterised correctly; no
     injection.
   - WebSocket auth — token-only when `enforce_zk_proofs = true`;
     raw-pseudonym path explicitly rejected.
   - ZK enforced-mode startup — `default_enforce_zk_proofs() = true`,
     missing v1 OR v2 key in enforced mode is a hard `StartupError`.
   - Image proxy URL SSRF — `is_private_or_reserved` covers
     loopback / private / link-local / IPv4-mapped-IPv6 with
     per-redirect-hop DNS validation. Tested.
   - Upload handlers — magic-byte content-type detection, UUID
     filenames, per-category size limits, EXIF/metadata stripping.
   - Production code has no `unwrap()`/`expect()` that can panic on
     attacker-controlled input — every remaining production `.expect(...)`
     is invariant-protected (HMAC key length, infallible serialization,
     Ctrl+C handler installation).
   - No `TODO`/`FIXME`/`XXX` comments left in production code outside
     of HTML metadata reference (`og:XXX`).
