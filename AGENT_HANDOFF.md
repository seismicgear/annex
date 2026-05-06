# Agent Handoff

## Current branch
`claude/fix-annex-bugs-PxyqS` (current session; previous session was `claude/fix-annex-bugs-itXFq`)

## Session goal
Recursive production bug-fix campaign on Annex (Tauri desktop, Rust workspace,
Groth16/Circom ZKP, SQLite). Fix highest-impact real bugs in priority order:
ZK enforcement → ZK release artifact path → canonical hex → Merkle epoch/concurrency
→ nullifier privacy → desktop release → security sweep.

## Fixed in this session (claude/fix-annex-bugs-PxyqS)

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

- [ ] **Federation v1/v2 dispatch**. `services/federation_service.rs` always
  uses `state.membership_vkey` (v1 vkey) regardless of what version the
  federated peer sent. v2 federation is not yet wired. Documented as an
  open follow-up. Low impact today (no v2 client), but a release blocker
  for the v2 rollout window.

- [ ] **CORS in debug builds bypasses configured origins on `localhost`**.
  `http/cors.rs::is_dev_localhost_origin` is gated on `cfg!(debug_assertions)`
  so release binaries are unaffected, but a misconfigured release build
  would silently accept any localhost origin. Worth a config flag rather
  than a cfg gate. Low priority given the gate is correct for normal usage.

- [ ] **Trusted-setup ceremony is single-machine, dev-fixture entropy**.
  `manifest.json` for membership pins SHA-256 hashes of artifacts produced
  by `dev-setup-groth16.js`, marked `ceremony.type: dev-fixture`. Real
  public release MUST replace these with multi-party ceremony output.
  Out of scope for this session.

- [ ] **PoT depth ceiling**. Current `pot14_*.ptau` is depth 14. The v1
  membership circuit fits in ~5k constraints, but if the circuit grows past
  ~16k constraints the script will need a depth-15+ PoT.

- [ ] **Federation peer-supplied URL not SSRF-guarded**. When
  `receive_federated_message` makes the freshness callback to
  `{originating_server}/api/federation/vrp-root`, the URL comes from the
  peer-signed envelope. Peers are administratively trusted via
  `federation_agreements`, but an explicit "is this URL public-routable"
  check would be defence-in-depth (matches what
  `api_link_preview::is_private_or_reserved` does). The federation HTTP
  client has redirect=none and timeouts, so the worst case is bounded.

- [ ] **Desktop build cannot be exercised in this environment** — system
  GTK/WebKitGTK packages are missing from the sandbox. Code review of
  `crates/annex-desktop/src/main.rs`, `embedded_server.rs`, and
  `tauri.conf.json` shows correct vkey/asset path resolution and bundle
  resource declarations. Real CI must keep enforcing the existing
  `release-desktop.yml` Linux + Windows jobs.

## Commands run (this session, claude/fix-annex-bugs-PxyqS)
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

## Context cutoff note
Session ran the full priority checklist with two new fixes (image
proxy SVG XSS, agent-handshake hijack) plus CI clippy-warning cleanup.
If a future agent picks up:
1. Re-run `cargo test --workspace --exclude annex-desktop` to confirm
   the **564-pass** baseline holds.
2. Re-run `cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings`
   to confirm CI clippy gate stays clean.
3. Highest-value remaining items are in "Still broken / suspected":
   - federation v1/v2 vkey dispatch (release blocker for v2 rollout)
   - federation peer URL SSRF guard (defence-in-depth)
   - real multi-party ZK ceremony (release blocker for v0.2)
   - desktop build smoke test in real Linux/Windows CI
4. Next concrete files to inspect:
   - `crates/annex-server/src/services/federation_service.rs` (vkey dispatch)
   - `crates/annex-federation/src/handshake.rs` (envelope evolution)
   - `zk/scripts/setup-groth16.js` and `dev-setup-groth16.js`
5. Areas already audited this session that look clean:
   - WS connection_manager lock ordering — no deadlock potential.
   - Channel CRUD / message edit-window enforcement — ownership +
     time-window checks correct.
   - Rate limiter / sliding window — sound.
   - CORS / `is_dev_localhost_origin` — correctly gated on
     `cfg!(debug_assertions)`; release builds stay strict.
   - SQL building (`api_observe::get_events_handler`,
     `services/rtx_repository.rs`) — parameterised correctly; no
     injection.
   - WebSocket auth — token-only when `enforce_zk_proofs = true`;
     raw-pseudonym path explicitly rejected.
   - ZK enforced-mode startup — `default_enforce_zk_proofs() = true`,
     missing key in enforced mode is a hard `StartupError`.
   - Image proxy URL SSRF — `is_private_or_reserved` covers
     loopback / private / link-local / IPv4-mapped-IPv6 with
     per-redirect-hop DNS validation. Tested.
