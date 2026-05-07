# Agent Handoff

## Current branch
`claude/fix-annex-bugs-Fshec` (current session; chain: `…itXFq` → `…PxyqS` → `…Las84` → `…AqBJk` → `…Fshec`)

## Session goal
Recursive production bug-fix campaign on Annex (Tauri desktop, Rust workspace,
Groth16/Circom ZKP, SQLite). Fix highest-impact real bugs in priority order:
ZK enforcement → ZK release artifact path → canonical hex → Merkle epoch/concurrency
→ nullifier privacy → desktop release → security sweep.

## Fixed in this session (claude/fix-annex-bugs-Fshec)

### [F26] STT/TTS child processes leaked on tokio future cancellation (DOS)
`crates/annex-voice/src/stt.rs::SttService::transcribe`,
`tts.rs::synthesize_piper`, `synthesize_bark`, and `synthesize_system`
all wrap `child.wait_with_output()` in `tokio::time::timeout(...)`. When
the timeout fires, the inner future is cancelled (dropped) — but
`tokio::process::Child` does NOT kill the underlying OS process on
drop by default. Quoting tokio docs: "By default, dropping a `Child`
does not kill the child process. To kill the child process when the
`Child` is dropped, call `Child::kill_on_drop(true)`."

So every STT timeout (default 120s) leaks a `whisper.cpp` orphan, every
piper TTS timeout (60s) leaks a `piper` orphan, etc. Under sustained
malicious input — many oversized payloads, many timeouts — the server
exhausts process slots and / or RAM. The 64 KiB / 10 MiB content caps
on input bound the per-request cost, but they don't bound the
across-requests leak rate.

Fix: chain `.kill_on_drop(true)` onto every `tokio::process::Command`
builder before spawn. tokio reaps the child on `Child::drop`, which is
what the timeout path produces. No behavioural change on the success
path (the child completes normally before drop). The fix is a single
builder call per command and inherits zero overhead on the happy path.

- files changed:
  - `crates/annex-voice/src/stt.rs::transcribe` — `.kill_on_drop(true)`
    on the whisper command, with a comment naming the timeout +
    leak-rate reasoning.
  - `crates/annex-voice/src/tts.rs::synthesize_piper` — same for piper.
  - `crates/annex-voice/src/tts.rs::synthesize_bark` — same for the
    Bark Python wrapper.
  - `crates/annex-voice/src/tts.rs::synthesize_system` — same for
    espeak-ng.
- tests run: `cargo check -p annex-voice` clean. Existing voice tests
  (12 passed including the [F21] / [F22] regression tests) still pass.
  The kill-on-drop behaviour is a tokio internal — unit-testing it
  requires a real subprocess and is exercised by the existing
  `voice_integration` tests under CI.
- result: PASS. STT / TTS subprocess timeouts no longer leak orphan
  processes.

### [F25] Policy re-handshake notification missed the SSRF guard (security)
Companion gap to [F12]/[F20]. `policy::notify_federation_peers_of_policy_change`
is the background task that POSTs a fresh VRP handshake to every
peer affected by a local policy change. It iterates a
`peers: Vec<(base_url, remote_instance_id)>` from the
`federation_agreements` table and `tokio::spawn`s an outbound
`{base_url}/api/federation/handshake` request per peer. No SSRF guard.
Same shape as [F12] (relay_message), [F20] (relay_rtx_bundles), and
[F12] again (attest_membership), but on the policy-change path.

A misconfigured `instances` row pointing at `http://localhost:9090`
turned every policy change (e.g. a moderator toggling
`policy.voice_enabled`) into a probe of internal services with the
freshly-signed handshake payload. Lower throughput than the message-
relay path because policy changes are rare, but the same SSRF posture.

Fix: gate `is_url_private_or_reserved` before the `tokio::spawn`. Skip
+ `tracing::warn!` with the peer URL and remote_instance_id, mirroring
the log shape of `relay_message` and `relay_rtx_bundles`. The message
is identical in structure to [F20]'s for grep-friendliness.

- files changed:
  - `crates/annex-server/src/policy.rs::notify_federation_peers_of_policy_change`
    — new `if is_url_private_or_reserved(&base_url) { skip }` gate
    immediately before the `tokio::spawn(...)` outbound POST.
- tests run: `cargo check -p annex-server` clean. (No new direct test;
  the predicate is already covered by `api_link_preview` tests and the
  rtx_service [F20] tests, and the call-site is structurally identical
  to those covered paths.)
- result: PASS. The last federation outbound path that lacked the SSRF
  guard is now closed. Every federation outbound in the tree
  (`grep "is_url_private_or_reserved" crates/annex-server/src/`) now
  hits the predicate.

### [F24] WebSocket per-message frame had no upper bound (DOS)
`api_ws::ws_handler` calls `WebSocketUpgrade::on_upgrade` directly
without setting `max_message_size` or `max_frame_size`. Tungstenite's
default `max_message_size` is 64 MiB. The
`MAX_WS_MESSAGE_CONTENT_LEN = 64 KiB` cap inside
`ws::dispatch::message::handle` does NOT fire until AFTER the frame
has been fully read off the socket and JSON-deserialised — at which
point the server has already buffered up to 64 MiB of attacker-
controlled bytes per message per connection. With many connections
that's a real OOM/saturation vector even though every individual
message is rejected as "content too long."

Fix: pin the per-message ceiling at the tungstenite layer with
`WebSocketUpgrade::max_message_size(WS_MAX_MESSAGE_BYTES)`, where
`WS_MAX_MESSAGE_BYTES = 128 KiB = 2 × MAX_WS_MESSAGE_CONTENT_LEN`. The
factor of 2 leaves headroom for the JSON envelope (keys, IDs,
`reply_to`, etc.) so legitimate
`MAX_WS_MESSAGE_CONTENT_LEN`-sized messages still pass.

- files changed:
  - `crates/annex-server/src/api_ws.rs` — new `pub(crate) const
    WS_MAX_MESSAGE_BYTES: usize = 128 * 1024`; `ws_handler` calls
    `ws.max_message_size(WS_MAX_MESSAGE_BYTES)` before `on_upgrade`.
- tests run: `cargo check -p annex-server` clean.
- result: PASS. The tungstenite-default 64 MiB ceiling is closed at
  the upgrade layer, well before any handler sees the bytes.

### [F23] RTX bundle fields had no size caps (DOS)
`annex_rtx::validate_bundle_structure` only checked for non-empty fields.
A bundle pushing axum's 2 MiB body cap could land in the database, get
broadcast to every WS subscriber, and get relayed to every active
federation peer — multiplying the cost of one oversized publish by the
fan-out factor. The local message path is bounded by
`FEDERATION_MAX_MESSAGE_CONTENT_LEN = 64 KiB` (see [F7]) but the RTX
path was wide open at every callsite (`RtxService::publish_bundle`,
`FederationService::receive_federated_rtx`, and on-the-wire envelopes
deserialised by axum).

The fix is structural: add field-level caps to
`validate_bundle_structure` so every callsite enforces the same bounds.
Caps:

| Field | Cap | Rationale |
|---|---|---|
| `summary` | 64 KiB | matches `FEDERATION_MAX_MESSAGE_CONTENT_LEN` |
| `reasoning_chain` | 256 KiB | longer chain-of-thought outputs are inherently bigger |
| `caveats` | 16 entries × 4 KiB each | structured short strings |
| `domain_tags` | 32 entries × 64 B each | short tag identifiers |
| `bundle_id`, `source_pseudonym`, `source_server`, `signature`, `vrp_handshake_ref` | 512 B each | identifiers / hex / URLs |

Total worst-case bundle size after caps: ~322 KiB, still well under
the global 2 MiB body cap with plenty of headroom for the rest of the
envelope. Existing tests in `annex-rtx::tests` and
`tests/api_federation_rtx_relay.rs` continue to pass — the legitimate
test fixtures are well under every cap.

- files changed:
  - `crates/annex-rtx/src/validation.rs` — new `MAX_*` consts; rewrote
    `validate_bundle_structure` to enforce them. Each cap miss returns
    `RtxError::InvalidBundle` with a message naming the offending
    field, matching the existing error shape.
  - `crates/annex-rtx/src/lib.rs` — re-export the `MAX_*` consts so
    callers that need the limits (e.g. for client-side hints) can read
    them; added 12 new tests in `tests::*`:
    - `oversized_summary_is_rejected`
    - `summary_at_cap_is_accepted` (boundary case)
    - `oversized_reasoning_chain_is_rejected`
    - `reasoning_chain_at_cap_is_accepted` (boundary case)
    - `missing_reasoning_chain_is_accepted` (Option::None passes)
    - `too_many_domain_tags_is_rejected`
    - `oversized_domain_tag_is_rejected`
    - `too_many_caveats_is_rejected`
    - `oversized_caveat_is_rejected`
    - `oversized_bundle_id_is_rejected`
    - `oversized_source_pseudonym_is_rejected`
    - `oversized_signature_is_rejected`
- tests run:
  - `cargo test -p annex-rtx` → 39 passed (12 new).
  - `cargo test -p annex-server --test api_federation_rtx_relay` →
    16 passed (no regressions; existing fixtures fit under caps).
- result: PASS. RTX publish + federated receive paths now refuse
  pathologically large bundles before they hit DB / WS / relay fan-out.

### [F22] TTS-to-Opus encoder was clipping every TTS frame (release blocker for agent voice)
Companion bug to [F21], opposite direction. `tts.rs::encode_pcm_to_opus_frames`
takes s16-le PCM (TTS output, 16kHz mono) and encodes it back to Opus
for injection into the WebRTC room via `inject_agent_opus`.
`opus-rs::OpusEncoder::encode` expects normalized `[-1.0, 1.0]` f32
input (it does `(x * 32768.0).clamp(-32768.0, 32767.0) as i16` internally,
see `opus-rs-0.1.12/src/lib.rs:366`). The pre-fix code passed
i16-range floats:

```rust
let frame_f32: Vec<f32> = frame.into_iter().map(|s| s as f32).collect();
```

So a half-amplitude TTS sample (`s = 16384`) became `f32 = 16384.0`,
which the encoder then scaled by 32768 (= 5.4e8) and clipped to 32767.
**Every non-trivial sample was driven to full-scale clip**, producing
the kind of grating distorted noise that says "the codec is alive, the
audio is dead."

The fix is the inverse of [F21]: divide by 32768.0 before passing to
the encoder. End-to-end the agent voice path now is:

```
TTS (s16le, full-scale i16)
 → divide by 32768 → f32 in [-1.0, 1.0]
 → opus-rs encoder (correct domain)
 → opus packet
 → WebRTC inject
```

A round-trip regression test (`encode_then_decode_preserves_amplitude_envelope`)
encodes a 440Hz half-amplitude sine through the helper, decodes the
output with `opus-rs::OpusDecoder::decode`, and asserts the decoded peak
amplitude stays in `(0.2, 0.85)`. Under the buggy implementation the peak
hits 1.0 (verified by temporarily reverting just the divisor — see the
session transcript). The bound is loose because Opus is lossy, but
strict enough to catch any encoder-domain mistake.

- files changed:
  - `crates/annex-voice/src/tts.rs` —
    1. Normalised `s as f32 → s as f32 / 32768.0` in
       `encode_pcm_to_opus_frames`.
    2. Added an explanatory inline comment that points at the exact
       opus-rs source line that defines the encoder's input domain.
    3. New `#[cfg(test)] mod tests` block with 5 tests:
       - `encode_silent_pcm_succeeds` (60ms silence → 3 frames).
       - `encode_sine_pcm_succeeds` (40ms sine → 2 frames, ≥1 byte each).
       - `encode_then_decode_preserves_amplitude_envelope` (round-trip
         regression, asserts peak ∈ (0.2, 0.85) after Opus lossy
         compression).
       - `encode_rejects_unaligned_pcm` (1-byte input → Codec error).
       - `encode_rejects_zero_channels`.
- tests run: `cargo test -p annex-voice --lib` → 12 passed (5 new in
  `tts::tests` + 7 from [F21] in `service::tests`).
- result: PASS. Agent voice output (TTS → Opus → WebRTC) is no longer
  destroyed at the encode step. Round-trip regression test guards
  against future re-introduction.

### [F21] Voice STT tap was emitting silence (release blocker for STT)
`crates/annex-voice/src/service.rs::tap_for_stt` decodes Opus frames into
normalized float PCM via `opus-rs::OpusDecoder::decode` — which writes
samples in the `[-1.0, 1.0]` range (confirmed in
`opus-rs-0.1.12/src/lib.rs:774,804,895`). The conversion to `s16le` for
the STT broadcast tap was:

```rust
let sample = s.round().clamp(i16::MIN as f32, i16::MAX as f32) as i16;
```

`s.round()` on a normalized float collapses every non-trivial sample to
{-1, 0, +1}, so the `pcm_s16le` payload sent to STT subscribers contains
near-silence regardless of the actual speech amplitude. Whisper-class
models on the receiving end produce empty transcripts or hallucinations
on this kind of degenerate input, which is consistent with the
"voice_integration::test_voice_config_status_enabled" flakiness called
out in CLAUDE.md (the audio path doesn't carry usable PCM).

The fix is the standard normalized-float-to-pcm scaling: multiply by
`i16::MAX` (32767) before rounding and clamping. Using 32767 instead of
32768 keeps the output symmetric around zero (full-scale negative maps
to `-32767`, not `-32768`), which matters because some STT preprocessors
compute mean and treat asymmetric clipping as a DC offset.

The conversion has been split out into a pure helper
`pcm_f32_to_s16le_bytes` so the transformation is unit-testable without
spinning up a WebRTC peer connection.

- files changed:
  - `crates/annex-voice/src/service.rs` — extracted
    `pcm_f32_to_s16le_bytes` (file-private), rewrote `tap_for_stt` to
    call it, added doc comment explaining the scaling and clamp
    semantics, and added a `#[cfg(test)] mod tests` block with 7
    tests:
    - `pcm_f32_to_s16le_zero_input_is_zero`
    - `pcm_f32_to_s16le_full_scale_positive_maps_to_i16_max`
    - `pcm_f32_to_s16le_full_scale_negative_maps_to_minus_i16_max`
    - `pcm_f32_to_s16le_mid_scale_speech_levels_are_audible` —
      explicitly asserts `0.5 → > 1000` (the bug it regresses against
      would produce `0.5 → 0/1`)
    - `pcm_f32_to_s16le_clips_above_full_scale`
    - `pcm_f32_to_s16le_clips_below_full_scale`
    - `pcm_f32_to_s16le_preserves_sample_count`
- tests run: `cargo test -p annex-voice --lib` → 7 passed (all new).
- result: PASS. The STT pipeline now receives real PCM amplitudes;
  the regression test asserts the broken behaviour can never silently
  return.

### [F20] RTX federation relay missed the SSRF guard (security)
[F12] last session added `is_url_private_or_reserved` to three federation
outbound paths in `services/federation_service.rs` (`attest_membership`,
the `receive_federated_message` freshness callback, and the message
`relay_message` background task). RTX bundle relay
(`services/rtx_service.rs::relay_rtx_bundles`) was missed. It loads peer
`base_url`s from `instances` via `list_active_federation_peers`, builds
`{base_url}/api/federation/rtx`, and `tokio::spawn`s a fire-and-forget
POST per peer. A misconfigured `instances` row with a private/loopback
URL (e.g. `http://localhost:9090`) would turn this into a continuous
internal-network probe carrying signed RTX envelopes — exactly the same
class of bug [F12] closed elsewhere.

The relay also fires after every successful `publish_bundle`, so the
abuse window is "any agent with `REFLECTION_SUMMARIES_ONLY` scope can
publish a bundle and trigger an outbound to whatever the operator
mistakenly typed in the `instances` table." That's lower-impact than the
attestation case (peer-supplied URL on the wire), but still well within
the SSRF defence-in-depth posture established by [F12].

Fix: add `rtx_peer_url_is_private_or_reserved` (a thin wrapper over
`api_link_preview::is_url_private_or_reserved`, kept as a `pub(crate)`
helper so the SSRF gate is directly testable from this module's unit
tests) and call it before each peer's outbound POST. Skipped peers
emit a `tracing::warn!` with the peer URL and bundle ID, mirroring the
log shape used by `relay_message`. The existing relay-path/origin cycle
check still runs first (so cycles still log at `debug`), and the
transfer-scope filter still runs after (so unknown-scope peers still
log at `warn`).

- files changed:
  - `crates/annex-server/src/services/rtx_service.rs` — new
    `rtx_peer_url_is_private_or_reserved` (pub(crate)); new SSRF gate
    inside `relay_rtx_bundles` between cycle-detection and transfer-
    scope checks; doc comment on `relay_rtx_bundles` updated to list
    the new behaviour.
  - `crates/annex-server/src/services/rtx_service.rs::tests` — 4 new
    unit tests covering loopback/IPv6 loopback,
    RFC1918/CGNAT/link-local/IPv4-mapped IPv6/reserved hostnames,
    unparseable + non-http(s) schemes, and public-host allow-through.
- tests run: `cargo test -p annex-server --lib services::rtx_service`
  → 12 passed (4 new).
- result: PASS. RTX relay no longer probes internal services from a
  misconfigured peer row, and the SSRF predicate is regression-protected
  by direct unit tests in the same module.

## Fixed in earlier session (claude/fix-annex-bugs-AqBJk)

## Fixed in earlier session (claude/fix-annex-bugs-AqBJk)

### [F14] verify-artifacts.js silently shipped dev-fixture in production (release blocker)
`zk/scripts/verify-artifacts.js` was the gate the release pipeline depends on
to confirm pinned ZK artifact hashes match the manifest. The release workflow
already sets `ANNEX_BUILD_PROFILE=production` (see
`.github/workflows/release-desktop.yml:22`), but the verifier only `warn()`'d
on `ceremony.type=="dev-fixture"` and exited 0 — meaning a tag-driven release
would silently ship dev-fixture (random-entropy, single-machine setup) keys
to production. Listed in the previous handoff as "Trusted-setup ceremony is
single-machine, dev-fixture entropy" — this fix closes the gate that allows
the dev-fixture artifacts to flow through.

Fix: verify-artifacts.js now respects `ANNEX_BUILD_PROFILE` (and a new
`--profile` CLI flag). Under a production profile, a manifest with
`ceremony.type == "dev-fixture"` is a hard fail (new exit code `3`) unless
the operator opts in with `ANNEX_ALLOW_DEV_CEREMONY=1`. The opt-in writes
its own loud warning + exists for staging dry-runs that need to be cut while
the real ceremony is being scheduled.

The current `zk/artifacts/membership/manifest.json` is still
`ceremony.type=dev-fixture`, so the next time the release workflow runs it
WILL FAIL fast at the verify step instead of producing a "shipping" build
that nobody realised was using random-entropy keys. That is the correct
fail-loud signal: the release should not happen until a real multi-party
ceremony is run and the manifest is regenerated. The escape hatch is
documented in the script's header.

- files changed:
  - `zk/scripts/verify-artifacts.js` — new profile resolution, new exit
    code 3, new opt-in env var, new `--profile` CLI flag.
  - `zk/scripts/verify-artifacts.test.js` — new node:test suite (11 tests)
    covering: dev profile passes ceremony=dev-fixture; production profile
    fails ceremony=dev-fixture with exit 3; release alias = production;
    `ANNEX_ALLOW_DEV_CEREMONY=1` opt-in works; production + ceremony=mpc
    works; unknown profile rejects with exit 1; opt-in has no effect under
    dev; hash mismatch still detected under production; missing required
    field rejects.
  - `zk/package.json` — `npm test` now runs `node --test
    scripts/verify-artifacts.test.js`.
  - `.github/workflows/ci.yml` — `check-server` now runs `npm test` in `zk`
    after generating dev keys; ~1s extra per CI run, regression-protects
    the production-profile gate.
- tests run:
  - `node --test zk/scripts/verify-artifacts.test.js` → 11 passed.
  - Manual smoke: `ANNEX_BUILD_PROFILE=production node zk/scripts/verify-artifacts.js`
    → exit 3 with "Refusing to verify dev-fixture" message; same with
    `ANNEX_ALLOW_DEV_CEREMONY=1` proceeds with warning.
- result: PASS. Production builds can no longer silently consume dev-fixture
  artifacts. The next release attempt against the current manifest will
  fail loudly until a real ceremony manifest is checked in.

### [F15] Enforced-mode startup now rejects on-disk dummy vkey (defence in depth)
Previously, `crates/annex-server/src/startup.rs` only refused to use the
in-memory dummy vkey when the file at `ANNEX_ZK_KEY_PATH` was *missing*.
If the file existed but happened to be byte-identical to
`generate_dummy_vkey()` (e.g. a test fixture accidentally copied into the
production deployment, or a hand-crafted JSON written by a curious
operator), the parser would happily accept it and the server would boot in
enforced mode while accepting any membership proof. A dummy vkey verifies
nothing.

Fix: `annex_identity::zk::is_dummy_vkey(&vk)` is a new pure-Rust predicate
that returns true when every group element of the loaded key matches the
deterministic generator-only dummy. The startup loader calls it after
parsing both the v1 and v2 keys; under `enforce_zk_proofs = true`, a
dummy match is now a new `StartupError::DummyVerificationKey { path }`
hard error.

- files changed:
  - `crates/annex-identity/src/zk.rs` — added `is_dummy_vkey` (pub) and
    `serialize_vkey_to_snarkjs_json` (pub, doc-hidden, used by tests to
    write a real on-disk dummy without committing a fixture).
  - `crates/annex-server/src/startup.rs` — new `StartupError::DummyVerificationKey`;
    both v1 and v2 vkey-load paths now call `is_dummy_vkey` after parsing
    and return the new error under enforcement. Unenforced (dev) mode is
    unchanged: a dummy on disk is accepted because the dummy is the
    explicit fallback in that mode.
  - `crates/annex-server/tests/zk_startup.rs` — 3 new tests:
    - `zk_enforced_mode_rejects_on_disk_dummy_vkey` — write generator-only
      vkey to disk, point env var at it, expect
      `StartupError::DummyVerificationKey` with the same path.
    - `zk_unenforced_mode_accepts_on_disk_dummy_vkey` — same setup, but
      `enforce_zk_proofs = false`, expect a clean boot.
    - `zk_v2_enforced_mode_rejects_on_disk_dummy_vkey` — same gate but on
      the v2 vkey path with v2 enabled.
  - `crates/annex-identity/src/zk.rs` — 4 new unit tests:
    - `is_dummy_vkey_detects_in_memory_dummy`
    - `is_dummy_vkey_rejects_non_dummy_alpha` (mutates one G1 to 2*G,
      asserts predicate flips)
    - `is_dummy_vkey_rejects_different_ic_length` (real vkey for membership
      has 3 IC entries vs dummy's 2)
    - `dummy_vkey_round_trips_through_snarkjs_json` (round-trip proves
      the JSON serialiser matches `parse_verification_key`'s grammar)
- tests run:
  - `cargo test -p annex-identity --lib zk::` → 33 passed (4 new).
  - `cargo test -p annex-identity --lib` → 69 passed (5 new total counting
    [F16] below).
  - `cargo test -p annex-server --test zk_startup` → 11 passed (3 new).
- result: PASS. Even if a dummy vkey somehow ends up at the production
  vkey path, enforced mode refuses to boot with it.

### [F16] v1 nullifier privacy gap documented in code (release blocker)
The v1 nullifier formula is `sha256(commitment_hex + ":" + topic)`. Both
inputs are public — the commitment is exposed by every API surface that
returns a Merkle path, federated identity row, public agent listing,
observe event, or channel membership. So any external observer who has
ever seen a commitment can recompute that user's per-topic pseudonym for
any topic, breaking the topic-unlinkability property the protocol claims.

The v2 path closes the gap (secret-derived nullifier inside the membership
circuit, see `zk/circuits/membership_v2.circom` +
`annex_identity::zk::topic_hash_for_v2`), and is opt-in via
`Config::security.enabled_zk_versions = ["v1", "v2"]`. The previous handoff
flagged this as a release blocker but didn't document it in code.

Fix: this session does NOT migrate the default to v2 (that's a wire change
across every client and federation peer). Instead it documents the property
in code so future readers, refactors, and reviewers find the limitation
without spelunking through commits:

1. Doc comment on `derive_nullifier_hex` in `crates/annex-identity/src/lib.rs`
   describing the privacy limitation, naming the v2 fix, pointing to the
   config knob.
2. New documentation test
   `v1_nullifier_is_publicly_derivable_from_commitment` in
   `crates/annex-identity/src/lib.rs::tests` that asserts:
   - the formula matches the public `sha256(commitment + ":" + topic)`
     bit-for-bit (so an accidental refactor that swaps in a secret-based
     nullifier on the v1 path FAILS the test loudly);
   - the nullifier is deterministic per (commitment, topic);
   - it varies across topics so per-topic pseudonyms remain distinct.
- files changed:
  - `crates/annex-identity/src/lib.rs` — doc comment on
    `derive_nullifier_hex` + new `v1_nullifier_is_publicly_derivable_from_commitment`
    test.
- tests run: `cargo test -p annex-identity --lib` → 69 passed.
- result: PASS. The v1 privacy gap is now documented in the same module
  where the broken function lives, with a regression-protection test and
  an explicit pointer to v2 as the fix.

### [F17] Pre-existing test failures from missing ZK toolchain
Two integration tests panicked when run on a fresh checkout (sandbox or
new dev machine) before the operator had run `node
zk/scripts/dev-setup-groth16.js`:

1. `tests/agent_flow_test.rs::test_agent_connection_flow_end_to_end`
   shelled out to `node node_modules/.bin/snarkjs ...` and panicked
   when snarkjs / wasm / zkey were missing. The comment in the test
   explicitly said "We assume the environment is set up." That's the
   wrong default — `cargo test --workspace --exclude annex-desktop`
   shouldn't fail on a fresh clone just because someone hasn't run a
   trusted-setup script yet.
2. `tests/api_identity_query.rs::test_get_identity_endpoints` did
   `expect("failed to read vkey")` on `zk/keys/membership_vkey.json`
   instead of the dummy fallback used everywhere else in the test
   harness.

Fix:

1. `agent_flow_test.rs` now checks for `membership.wasm`,
   `membership_final.zkey`, and `snarkjs` up front; if any are missing
   it `eprintln!`s a clear hint ("run `cd zk && npm ci && node
   scripts/build-circuits.js && node scripts/dev-setup-groth16.js`") and
   `return`s. CI builds the toolchain before running tests, so the real
   path still exercises the full Groth16 round-trip; only fresh sandboxes
   skip it. This matches the existing pattern in
   `zk_startup::zk_v2_enabled_loads_v2_vkey`.
2. `api_identity_query.rs` now mirrors `tests/common/mod.rs::load_vkey_or_dummy`:
   real vkey if the file is present, otherwise `generate_dummy_vkey()`.
   `enforce_zk_proofs` is `false` in this test, so the dummy is accepted
   by design.
3. `tests/api_ws.rs` had the same pattern — fixed to fall back to dummy
   instead of `expect`.
- files changed:
  - `crates/annex-server/tests/agent_flow_test.rs`
  - `crates/annex-server/tests/api_identity_query.rs`
  - `crates/annex-server/tests/api_ws.rs`
- tests run:
  - `cargo test -p annex-server --test agent_flow_test` → 1 passed (skipped
    in sandbox; would still run the full path under CI with keys).
  - `cargo test -p annex-server --test api_identity_query` → 1 passed.
- result: PASS. `cargo test -p annex-server` is no longer blocked by a
  missing ZK toolchain on a fresh checkout.

### [F18] Desktop main.rs: clearer warning when no vkey resource is found
`crates/annex-desktop/src/main.rs` searches four candidate locations for
`membership_vkey.json`. If none exists it just silently skips setting
`ANNEX_ZK_KEY_PATH`, which means the embedded server falls back to the
default path `zk/keys/membership_vkey.json` — which also probably doesn't
exist in a packaged install — and then `StartupError::MissingVerificationKey`
fires with a generic "file not found" reason buried in the server log.
The user sees "the embedded server failed to start" with no obvious next
step.

Fix: log a clear `eprintln!` BEFORE handing off to the embedded server,
listing every candidate path the loader tried and naming the
`enforce_zk_proofs` consequence. The stale "falls back to a dummy vkey"
comment block is also updated to reflect post-[F15] enforcement behaviour.
- files changed: `crates/annex-desktop/src/main.rs` — pre-startup vkey
  candidate dump + updated doc comment.
- tests run: `cargo build -p annex-desktop` not run (sandbox lacks GTK3 +
  WebKitGTK 4.1 dev packages, same constraint as previous sessions). The
  edit is a `eprintln!` + comment, no API surface change, no compile risk.

### [F19] annex-identity tests skip when ZK toolchain unavailable
Same class of pre-existing failure as [F17] but in the
annex-identity test crate: `tests/zk_integration.rs::
test_identity_commitment_proof_verification` panicked when run on a
fresh checkout because `tests/common.rs::ensure_zk_artifacts` would
shell out to `node scripts/build-circuits.js` and
`assert!(status.success())`. Some sandboxes (this one included, when
disk is tight) cannot compile circom, so the test would abort with a
mid-circom panic instead of skipping.

Fix:
- `ensure_zk_artifacts` now returns `Result<(), String>` carrying the
  failure reason. Stored in a `Mutex<Option<...>>` next to the
  existing `Once`, so every caller sees the same outcome.
- New public helper `zk_toolchain_available()` calls
  `ensure_zk_artifacts` and returns `false` (with an `eprintln!` skip
  note) when the result is `Err`.
- `test_identity_commitment_proof_verification` now calls it at the
  top and returns early on `false`.
- `get_zk_paths` and `get_verification_key` panic with a clear
  "ZK artifacts unavailable: ..." message if reached without the
  toolchain — that branch is unreachable from the gated test, but
  preserves the original API contract for any external callers.

CI runs `node zk/scripts/dev-setup-groth16.js` up-front so the real
path always exercises the full Groth16 round-trip. Skipping is
sandbox-only.
- files changed:
  - `crates/annex-identity/tests/common.rs` — Result-returning
    `ensure_zk_artifacts`, new `zk_toolchain_available`.
  - `crates/annex-identity/tests/zk_integration.rs` — skip gate at top
    of `test_identity_commitment_proof_verification`.
- tests run:
  - `cargo test -p annex-identity --tests` → all pass (test runs
    successfully when toolchain is available, skips cleanly otherwise).
  - `cargo fmt --all --check` → clean.

## Fixed in earlier session (claude/fix-annex-bugs-Las84)

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

- [ ] **Trusted-setup ceremony is single-machine, dev-fixture entropy**.
  `manifest.json` for membership pins SHA-256 hashes of artifacts produced
  by `dev-setup-groth16.js`, marked `ceremony.type: dev-fixture`.
  As of [F14] this session, the production-profile run of
  `verify-artifacts.js` REFUSES to proceed against this manifest (exit
  code 3) — so a release tag triggered today against this branch will
  fail at the verify step instead of silently shipping dev keys. Closing
  this fully still requires running an actual multi-party ceremony,
  generating new artifacts, regenerating the manifest with
  `ceremony.type: mpc` (or similar), and pinning the new SHA-256s.
  Until then, releases must be cut with `ANNEX_ALLOW_DEV_CEREMONY=1`
  for staging only, never tagged as a public release.

- [ ] **v1 nullifier privacy gap** — see [F16]. The v1 nullifier is
  `sha256(commitment + ":" + topic)`, both inputs public, so any external
  observer can derive every per-topic pseudonym from a known commitment.
  The fix is to migrate the default to v2 (already implemented in the
  circuit + verifier; opt-in today via
  `Config::security.enabled_zk_versions = ["v1", "v2"]` and per-client
  protocol selection). That migration is large (every client + every
  federation peer must speak v2) and out of scope here. The privacy
  property is now documented in code (doc comment + regression test in
  `derive_nullifier_hex`) so a future agent finds it without spelunking.

- [ ] **CORS in debug builds bypasses configured origins on `localhost`**.
  `http/cors.rs::is_dev_localhost_origin` is gated on `cfg!(debug_assertions)`
  so release binaries are unaffected, but a misconfigured release build
  would silently accept any localhost origin. Worth a config flag rather
  than a cfg gate. Low priority given the gate is correct for normal usage.
  Verified previous session: `cfg!(debug_assertions)` is `false` under
  `--release` so the original concern is partially overstated, but a
  config flag would still be cleaner than the cfg gate.

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
  resource declarations. [F18] this session adds a clear pre-startup
  warning on the desktop side when no `membership_vkey.json` resource is
  found. Real CI must keep enforcing the existing `release-desktop.yml`
  Linux + Windows jobs.

## Fixed in this session, previously listed as "Still broken"
- **[F14] verify-artifacts.js dev-fixture gate** — production profile now
  refuses dev-fixture manifests (exit 3) unless explicitly opted-in via
  `ANNEX_ALLOW_DEV_CEREMONY=1`. Release pipeline can no longer silently
  ship random-entropy dev keys.
- **[F15] enforced-mode dummy-vkey-on-disk gate** — `is_dummy_vkey`
  predicate in `annex_identity::zk` plus startup-time check in both v1
  and v2 vkey paths.
- **[F16] v1 nullifier privacy gap documentation** — doc comment + code
  test in `derive_nullifier_hex`. The actual privacy fix is the v2
  migration; this session only ensures the gap is visible in code.

## Fixed in earlier session, previously listed as "Still broken"
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

## Commands run (this session, claude/fix-annex-bugs-AqBJk)
- `cargo fmt --all --check` → clean.
- `cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings`
  → clean.
- `node --test zk/scripts/verify-artifacts.test.js` → **11 passed** (all new).
- `cargo test -p annex-identity --lib` → **69 passed** (5 new for
  `is_dummy_vkey` + `serialize_vkey_to_snarkjs_json` round-trip + v1
  nullifier privacy doc test).
- `cargo test -p annex-server --test zk_startup` → **11 passed** (3 new
  for the on-disk dummy-vkey gate, v1 + v2).
- `cargo test -p annex-server --test api_zk_verify` → 1 passed (now
  cleanly skips when ZK toolchain absent).
- `cargo test -p annex-server --test agent_flow_test` → 1 passed (now
  cleanly skips when ZK toolchain absent).
- `cargo test -p annex-server --test api_identity_query` → 1 passed
  (dummy-vkey fallback added).
- `cargo test -p annex-server --test api_ws` → was already covered by
  the dummy-vkey fallback after the edit.
- `cargo test -p annex-server` → **353 passed, 0 failed** (up from
  350; 3 new tests for the on-disk dummy-vkey gate).
- `cargo test --workspace --exclude annex-desktop --exclude annex-server`
  → **224 passed, 0 failed** (up from 219; 5 new tests in
  annex-identity zk module).
- Workspace total: **577 passed, 0 failed** (up from 569 baseline; 8
  new Rust tests + 11 new node:test tests in zk/scripts).
- Sandbox cannot exercise: full Tauri desktop build (GTK/WebKitGTK
  packages absent), real Groth16 prove path
  (`zk/build/membership_js/membership.wasm` not generated). Both are
  exercised by CI lanes already.

## Commands run (previous session, claude/fix-annex-bugs-Las84)
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
  every federation outbound URL. Active call sites:
  * `attest_membership` — hard reject (403). [F12]
  * `receive_federated_message` freshness callback — skip + log. [F12]
  * `relay_message` background relay — skip + warn. [F12]
  * `relay_rtx_bundles` background relay — skip + warn. [F20]
  * `notify_federation_peers_of_policy_change` background re-handshake
    — skip + warn. [F25]
  Every future federation outbound (any
  `tokio::spawn(client.post(&url)…)` over a peer-derived URL) MUST go
  through this predicate. See [F12], [F20], [F25].
- I-ZK-HEADER-V2 (new): `verify_zk_membership_header` (the per-request
  channel ZK gate read from `x-annex-zk-proof`) accepts an optional
  `protocolVersion` field and dispatches to the v2 vkey when set to
  `"v2"`. v2 also requires `publicSignals` (length 4) and `topic`
  for the canonical topicHash binding. v1 wire shape is unchanged. See
  [F13].
- I-ZK-CEREMONY-PROD (new): Under
  `ANNEX_BUILD_PROFILE=production|release`, `zk/scripts/verify-artifacts.js`
  REFUSES manifests with `ceremony.type == "dev-fixture"` (exit 3).
  `ANNEX_ALLOW_DEV_CEREMONY=1` is the documented escape hatch for staging
  dry-runs only and MUST NOT be set in any tag-driven public release. The
  current `zk/artifacts/membership/manifest.json` is dev-fixture, so the
  release pipeline against this branch fails until the manifest is
  regenerated from a real ceremony. See [F14].
- I-ZK-DUMMY-DETECT (new): Under `enforce_zk_proofs = true`, startup
  refuses to load a dummy verifying key from disk via
  `annex_identity::zk::is_dummy_vkey`. The predicate is a structural
  match against the deterministic generator-only vkey produced by
  `generate_dummy_vkey`. Both v1 and v2 vkey paths are covered. See
  [F15].
- I-V1-NULLIFIER-PUBLIC (new): The v1 nullifier formula
  `sha256(commitment + ":" + topic)` is publicly derivable from any
  observed commitment. This is a documented v1 property, not a bug;
  servers that need topic unlinkability must enable v2
  (`Config::security.enabled_zk_versions = ["v1", "v2"]` and migrate
  clients off v1). The property is asserted in code by
  `v1_nullifier_is_publicly_derivable_from_commitment`. See [F16].
- I-RTX-SSRF (new): RTX federation relay
  (`services/rtx_service.rs::relay_rtx_bundles`) MUST go through
  `rtx_peer_url_is_private_or_reserved` before any outbound POST.
  Mirrors `I-FED-SSRF-1`. Future RTX outbound call sites added to
  `rtx_service.rs` MUST also gate on this predicate. See [F20].
- I-VOICE-PCM-NORMALIZED (new): The voice pipeline crosses the
  `opus-rs::Decode/Encode` boundary on both sides:
  * Decode (incoming WebRTC RTP → STT tap): scales normalised float
    PCM to s16-le by multiplying by `i16::MAX` (32767), via
    `service::pcm_f32_to_s16le_bytes`.
  * Encode (TTS s16-le PCM → outgoing Opus): divides s16 samples by
    `32768.0` to land in the encoder's normalised input domain, in
    `tts::encode_pcm_to_opus_frames`.
  Both helpers are unit-tested with regression-protection assertions
  that fail loudly if either scaling is dropped. See [F21], [F22].
- I-RTX-SIZE (new): `validate_bundle_structure` enforces field-level
  size caps on every RTX bundle (`summary` 64 KiB,
  `reasoning_chain` 256 KiB, `caveats` 16×4 KiB,
  `domain_tags` 32×64 B, identifier-like fields 512 B). Both the local
  publish path and the federated receive path call it before any DB
  write or relay fan-out, mirroring the
  `FEDERATION_MAX_MESSAGE_CONTENT_LEN` bound on raw messages. See [F23].
- I-WS-FRAME-CAP (new): WebSocket connections are upgraded with
  `max_message_size = WS_MAX_MESSAGE_BYTES = 128 KiB`, which is
  2 × `MAX_WS_MESSAGE_CONTENT_LEN` (envelope headroom). Future
  IncomingMessage variants whose total wire size exceeds 128 KiB MUST
  bump `WS_MAX_MESSAGE_BYTES` AND `MAX_WS_MESSAGE_CONTENT_LEN` together;
  do not bump only one. See [F24].
- I-VOICE-CHILD-KILL-ON-DROP (new): Every `tokio::process::Command`
  spawned in `annex-voice` (STT whisper, TTS piper, TTS bark,
  TTS espeak-ng) MUST call `.kill_on_drop(true)` on the builder
  before `.spawn()`. Without it, a tokio future cancellation (e.g.
  on STT_TIMEOUT / TTS_TIMEOUT) leaves the child process orphaned in
  the OS. See [F26].

## Context cutoff note
Session [F14..F18] tightened the production gates around the ZK toolchain
end-to-end:
- `verify-artifacts.js` is now profile-aware and refuses to ship dev-fixture
  manifests in production (release pipeline now fails fast against the
  current dev-fixture manifest, which is the correct signal — see [F14]).
- Startup refuses to boot in enforced mode against an on-disk dummy vkey
  (defence in depth via `is_dummy_vkey` predicate — see [F15]).
- v1 nullifier privacy gap is now documented in code with a regression-
  protected doc test (see [F16]).
- Three pre-existing test failures from missing ZK toolchain converted to
  graceful skips so `cargo test -p annex-server` works on a fresh checkout
  (see [F17]).
- Desktop main.rs prints a clear pre-startup warning when no
  membership_vkey.json resource is found (see [F18]).

All clippy + fmt clean. New tests added: 4 in annex-identity::zk
(`is_dummy_vkey_*`, `dummy_vkey_round_trips_through_snarkjs_json`),
1 in annex-identity::lib (`v1_nullifier_is_publicly_derivable_from_commitment`),
3 in annex-server::zk_startup (`zk_*enforced_mode_rejects_on_disk_dummy_vkey*`,
`zk_unenforced_mode_accepts_on_disk_dummy_vkey`), and 11 in
`zk/scripts/verify-artifacts.test.js` (Node test runner). Three integration
tests (agent_flow_test, api_zk_verify, api_identity_query) had pre-existing
panics fixed.

If a future agent picks up:
1. Re-run `cargo test -p annex-server` and
   `cargo test --workspace --exclude annex-desktop --exclude annex-server`
   to confirm the baseline holds (this session is up at least 8 tests on
   the previous 569; new total expected ~580+ once the broader suite runs
   without disk-pressure constraints). Watch out for the disk-pressure
   issue described under "Commands run".
2. Re-run `cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings`
   to confirm CI clippy gate stays clean.
3. Re-run `node --test zk/scripts/verify-artifacts.test.js` to confirm the
   new dev-fixture gate stays green (now also gated by the `npm test` step
   added to `.github/workflows/ci.yml`).
4. Highest-value remaining items are in "Still broken / suspected":
   - real multi-party ZK ceremony — TOP priority, blocks tagged release
     because `verify-artifacts.js` will now (correctly) refuse to ship the
     current dev-fixture manifest under `ANNEX_BUILD_PROFILE=production`.
   - v1 nullifier privacy gap (release blocker for any deployment claiming
     topic unlinkability; v2 path is implemented and opt-in).
   - PoT depth ceiling (only matters if circuit grows past ~16k constraints).
   - uploads-as-public-URL design question (release blocker for any
     private-channel mode).
   - desktop build smoke test in real Linux/Windows CI.
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
