# ANNEX Platform Security Audit Report

**Auditor:** Claude (Automated Hostile Audit)
**Date:** 2026-03-06
**Scope:** Full platform — identity plane, VRP trust protocol, ZK circuits, federation, WebSocket transport, database, server API
**Attacker Models:** Rogue Server Operator, Network Observer, Disk Thief, Rogue Agent, Federation Attacker

---

## Executive Summary

The ANNEX platform demonstrates strong architectural intent: zero-knowledge identity, cryptographic federation, and value-aligned agent participation. However, the implementation has several critical security gaps that undermine its core invariants. The most severe: ZK proof enforcement is **off by default**, nullifiers are derivable from public data, the CORS layer blocks the ZK proof header, and the WebSocket transport accepts raw pseudonyms without cryptographic binding.

**Finding Count:** 11 CRITICAL, 13 HIGH, 7 MEDIUM, 4 LOW, 3 NOTE (across 2 audit passes)

---

## CRITICAL Findings

### [CRITICAL] FINDING-001: ZK Proof Enforcement Defaults to OFF

**File:** `crates/annex-server/src/config.rs:47`, `crates/annex-server/src/lib.rs:619`
**Attacker:** Rogue Server Operator, Network Observer
**Category:** Identity Plane

**Description:** The `enforce_zk_proofs` configuration defaults to `false`. This means every deployment starts with ZK proof verification disabled unless the operator explicitly enables it. The `verify_zk_membership_header` function in `middleware.rs:450` short-circuits with `Ok(())` when enforcement is off, making the entire ZK identity plane a no-op.

**Impact:** Any entity can claim any pseudonym without proving Merkle tree membership. The core invariant "No identity without proof" is violated by default. An attacker who knows (or guesses) a pseudonym string can impersonate that identity.

**Reproduction:**
1. Start server with default config (no `security.enforce_zk_proofs = true`)
2. Send any API request with `Authorization: Bearer <any-pseudonym-in-db>`
3. Request is accepted without ZK proof

**Fix:** Change the default to `true`. Deployments that have not completed trusted setup can explicitly opt out.

**Verification:** Unit test confirms default config has `enforce_zk_proofs: true`.

---

### [CRITICAL] FINDING-002: CORS Configuration Blocks ZK Proof Header

**File:** `crates/annex-server/src/lib.rs:979-983`
**Attacker:** Network Observer (via browser client)
**Category:** Identity Plane

**Description:** The CORS `allow_headers` list includes `content-type`, `authorization`, and `x-annex-pseudonym`, but does NOT include `x-annex-zk-proof`. When CORS is enabled (any cross-origin deployment), browsers will block preflight requests that include the ZK proof header, making it impossible for browser clients to submit ZK proofs.

**Impact:** Browser-based clients cannot use ZK proof verification at all when CORS is active. This silently degrades the security model from "cryptographic proof" to "bearer token".

**Fix:** Add `x-annex-zk-proof` to the CORS `allow_headers` list.

---

### [CRITICAL] FINDING-003: Nullifier Derived from Public Commitment, Not Secret Key

**File:** `crates/annex-identity/src/lib.rs:140`
**Attacker:** Any (public knowledge attack)
**Category:** Cryptographic, Identity Plane

**Description:** The nullifier derivation formula is `sha256(commitmentHex + ":" + topic)`. The commitment is a public value (stored in the database, returned by APIs, and a public output of the ZK circuit). This means **anyone** who knows a commitment can compute the nullifier for any topic, and therefore compute the pseudonym via `sha256(topic + ":" + nullifierHex)`.

**Impact:** The pseudonym derivation is deterministic from public data. An observer who collects commitments can link all pseudonyms across all topics back to the same commitment. This violates the privacy design goal of topic-scoped unlinkability. The README specification states this formula, so this is a **design flaw**, not an implementation bug.

**Fix:** This is an architectural issue. The nullifier should be derived from the secret key: `nullifier = sha256(sk + ":" + topic)` and proved in-circuit. This requires a new Circom circuit. As an interim measure, document the limitation prominently and ensure commitments are not exposed unnecessarily.

---

### [CRITICAL] FINDING-004: WebSocket Accepts Raw Pseudonym Without Cryptographic Binding

**File:** `crates/annex-server/src/api_ws.rs:481-487` (line references from persisted output)
**Attacker:** Network Observer, Rogue Agent
**Category:** Communication Plane

**Description:** The WebSocket handler accepts a `?pseudonym=<value>` query parameter as a "legacy" auth path. This requires no HMAC token, no ZK proof — just knowledge of the pseudonym string. Although `?token=` is preferred, the legacy path is still active and there is no configuration to disable it.

**Impact:** Any entity that knows a pseudonym (which, per FINDING-003, is computable from public data) can open a WebSocket connection as that identity, receiving all real-time messages and sending messages as that user.

**Fix:** Add a config flag to disable the legacy pseudonym parameter. When `enforce_zk_proofs` is true, reject raw pseudonym connections.

---

### [CRITICAL] FINDING-005: Auth Middleware Uses Pseudonym as Bearer Token

**File:** `crates/annex-server/src/middleware.rs:32-47`
**Attacker:** Network Observer, Rogue Agent
**Category:** Authentication

**Description:** The `auth_middleware` accepts the pseudonym string directly as a Bearer token. There is no signature, no HMAC, no proof-of-possession. The middleware comment explicitly acknowledges this: "In this phase (Phase 2), authentication relies on the pseudonym acting as a bearer token."

Combined with FINDING-003 (pseudonyms are computable from public commitments), this means the entire REST API auth is effectively open to anyone who can observe or compute a pseudonym.

**Impact:** Complete impersonation of any registered identity via REST API.

**Fix:** Require HMAC-signed session tokens (similar to the WS token flow) for REST API auth. The `ws_token_secret` infrastructure already exists.

---

### [CRITICAL] FINDING-006: Auto-Promotion to Founder When Moderators Are Stale

**File:** `crates/annex-server/src/api.rs:632-672`
**Attacker:** Rogue Agent
**Category:** Authorization, Privilege Escalation

**Description:** The `fetch_platform_identity` helper checks if all existing moderators are "stale" (no `graph_node.last_seen_at` within 5 minutes). If so, it promotes the **requesting identity** to full admin (can_voice, can_moderate, can_invite, can_federate). Any identity — including AI agents — can trigger this simply by calling `GET /api/identity/:pseudonymId` when no moderator has been active recently.

**Impact:** Privilege escalation to full admin. A rogue agent can wait for the human moderator to go offline for 5 minutes, then call the identity endpoint to become founder. With moderator privileges, the agent can modify server policy, invite other entities, and federate.

**Fix:** Remove the stale-moderator auto-promotion. Founder promotion should only happen via explicit admin action or a secure bootstrap flow, not automatically on any identity lookup.

---

### [CRITICAL] FINDING-007: Missing ZK Circuits and Trusted Setup Artifacts

**File:** `zk/circuits/`, `zk/keys/`, `zk/build/`
**Attacker:** All
**Category:** ZK Infrastructure

**Description:** The README specifies 5 circuits: `identity-commitment`, `membership-proof`, `channel-eligibility`, `link-pseudonyms`, and `federation-attestation`. Only 2 exist (`identity.circom`, `membership.circom`). The `zk/keys/` and `zk/build/` directories are empty — no trusted setup artifacts (proving keys, verification keys) exist. The server falls back to a `generate_dummy_vkey()` which makes all proof verifications fail.

**Impact:** ZK proofs cannot be generated or verified. The identity plane is entirely non-functional from a cryptographic standpoint. The 3 missing circuits mean channel-scoped access, cross-topic pseudonym linking, and federation attestation have no ZK backing.

**Fix:** This is a build infrastructure gap. At minimum, provide a script to run trusted setup and generate keys. Document which circuits are implemented vs. planned.

---

## HIGH Findings

### [HIGH] FINDING-008: `from_be_bytes_mod_order` Used Without Reduction Validation

**File:** `crates/annex-identity/src/zk.rs:53`, `crates/annex-identity/src/merkle.rs:210`, `crates/annex-identity/src/registry.rs:80`
**Attacker:** Rogue Agent, Federation Attacker
**Category:** Cryptographic

**Description:** `Fr::from_be_bytes_mod_order()` silently reduces inputs modulo the BN254 scalar field order. If an input is >= the field modulus, two different byte strings map to the same field element. In `zk.rs:53` (`parse_fr_from_hex`), this means a malicious client could submit a hex value >= modulus that gets silently reduced, creating ambiguity. In `merkle.rs:210` and `registry.rs:80`, the same issue during tree restoration/insertion could cause Merkle root mismatches.

Note: `commitment.rs:36-49` correctly validates this via roundtrip check for the `sk` input, but the other call sites lack this validation.

**Impact:** Potential for commitment collisions in the Merkle tree, Merkle root desynchronization, and proof verification against wrong field elements.

**Fix:** Add reduction validation (roundtrip check) to `parse_fr_from_hex`, `merkle.rs:210`, and `registry.rs:80`.

---

### [HIGH] FINDING-009: Verify Membership Handler Accepts Any Historical Root

**File:** `crates/annex-server/src/api.rs:335-349`
**Attacker:** Rogue Agent
**Category:** Identity Plane

**Description:** The `verify_membership_handler` checks `SELECT COUNT(*) FROM vrp_roots WHERE root_hex = ?1` — it only verifies the root exists in the table, not that it is the *current* root or marked as active. Old roots from before identity revocations remain valid forever.

**Impact:** A revoked identity whose commitment was in a historical Merkle tree can continue to generate valid proofs against that old root. Identity revocation does not invalidate existing proofs.

**Fix:** Filter `vrp_roots` by `active = 1` or only accept the current tree root. Alternatively, maintain a sliding window of recent roots (e.g., last N) and reject anything older.

---

### [HIGH] FINDING-010: Agent Handshake Does Not Validate Participant Type

**File:** `crates/annex-server/src/api_vrp.rs:27-30`
**Attacker:** Rogue Agent
**Category:** VRP Trust Protocol

**Description:** The `agent_handshake_handler` accepts any `pseudonym_id` and records it with `peer_type = "AI_AGENT"` (hardcoded at line 97). It does not verify that the pseudonym actually belongs to an entity registered with `RoleCode::AiAgent`. A human user could call this endpoint and get registered as an agent, or vice versa.

**Impact:** Role confusion. A human identity could gain agent capabilities (RTX publishing, voice profiles). An agent could bypass agent-specific restrictions by not handshaking.

**Fix:** Look up the identity's role_code from the database and reject if it doesn't match `RoleCode::AiAgent`.

---

### [HIGH] FINDING-011: WebSocket Token HMAC Comparison Is Not Constant-Time

**File:** `crates/annex-server/src/api_ws.rs:104` (persisted output line)
**Attacker:** Network Observer (timing side-channel)
**Category:** Cryptographic

**Description:** The WS token verification uses `expected_sig.as_slice() != provided_sig.as_slice()` for HMAC comparison. This is a byte-by-byte comparison that short-circuits on the first mismatch, leaking timing information about how many bytes match.

**Impact:** A network observer who can measure response timing could incrementally guess the HMAC signature byte-by-byte. Practical exploitation requires high-precision timing but is a well-known vulnerability class.

**Fix:** Use `hmac::Mac::verify_slice()` or a constant-time comparison function.

---

### [HIGH] FINDING-012: `repro_bytes.rs` Debugging Artifact in Repository Root

**File:** `repro_bytes.rs` (repository root)
**Attacker:** Disk Thief
**Category:** Information Disclosure

**Description:** A debugging file containing `ark_bn254` test code exists in the repository root. While not directly exploitable, it indicates debugging artifacts that may contain sensitive cryptographic test values, and it pollutes the production codebase.

**Impact:** Low direct impact, but indicates insufficient code hygiene. Could contain test keys or values that give an attacker insight into the cryptographic implementation.

**Fix:** Remove the file and add it to `.gitignore`.

---

## MEDIUM Findings

### [MEDIUM] FINDING-013: Redacted Topics Not Enforced in WebSocket Message Delivery

**File:** `crates/annex-server/src/api_ws.rs` (broadcast function)
**Attacker:** Rogue Agent
**Category:** VRP Trust Protocol

**Description:** The RTX publish handler correctly enforces `redacted_topics` from the agent's capability contract. However, the WebSocket `broadcast()` function delivers messages to all channel subscribers without checking whether the channel topic matches any agent's `redacted_topics`. An agent subscribed to a channel whose topic is in their redacted list will still receive all messages.

**Impact:** Agents receive messages from topics they agreed not to access, violating the VRP capability contract.

**Fix:** Check `redacted_topics` during WebSocket subscribe and/or broadcast.

---

### [MEDIUM] FINDING-014: Semantic Alignment Uses Bag-of-Words (No Real NLP)

**File:** `crates/annex-vrp/src/semantic.rs:14-68`
**Attacker:** Rogue Agent, Federation Attacker
**Category:** VRP Trust Protocol

**Description:** The `BagOfWordsEmbedder` is trivially gameable. An agent can include the exact same words as the server's principles in their own principles while meaning the opposite (e.g., "we do not engage in surveillance" vs "we engage in surveillance" both have high bag-of-words similarity). The cosine similarity of BoW vectors doesn't capture semantic meaning, negation, or intent.

**Impact:** Alignment scoring is unreliable. A malicious agent can craft principles that score ALIGNED while having completely opposite intentions.

**Fix:** Replace with a real embedding model (sentence-transformers, etc.) or document this as a known limitation. At minimum, add negation-aware tokenization.

---

### [MEDIUM] FINDING-015: VRP Federation Handshake Missing Specified Fields

**File:** `crates/annex-vrp/src/types.rs`
**Attacker:** Federation Attacker
**Category:** Federation Plane

**Description:** The README specification defines `VrpFederationHandshake` as including `protocol_version`, `identity_hash`, `signed_by`, and `signature` fields. The implementation lacks `protocol_version` and `identity_hash`. Without version negotiation, protocol upgrades become breaking changes. Without identity binding, handshake payloads can be replayed between different servers.

**Impact:** Federation handshakes are not bound to the claimed server identity and have no version negotiation.

**Fix:** Add the missing fields and validate them during handshake processing.

---

### [MEDIUM] FINDING-016: Migration Chain Has No Integrity Verification

**File:** `crates/annex-db/src/migrations.rs`
**Attacker:** Disk Thief
**Category:** Database/Storage

**Description:** Migrations are tracked by name in the `_migrations` table but no checksum is stored or verified. A Disk Thief who modifies a migration file after it was applied would not be detected. There is also no gap detection — if migration `005` is skipped, later migrations still apply.

**Impact:** Schema tampering goes undetected. Could be used to weaken security constraints (e.g., removing UNIQUE constraints on nullifiers).

**Fix:** Store SHA-256 checksums of applied migrations and verify on startup.

---

### [MEDIUM] FINDING-017: Membership Circuit Uses `var` for Mux Logic

**File:** `zk/circuits/membership.circom:31-32`
**Attacker:** Rogue Agent
**Category:** ZK Verification

**Description:** The Merkle proof mux computation uses `var` for `left` and `right`:
```
var left = pathIndexBits[i] * (pathElements[i] - currentHash[i]) + currentHash[i];
var right = pathIndexBits[i] * (currentHash[i] - pathElements[i]) + pathElements[i];
```
In Circom, `var` computations are unconstrained — they compute the correct value for witness generation but don't add constraints to the R1CS system. The constraints come from `poseidons[i].inputs[0] <== left` which constrains the *value* but not the *computation path*. However, since `pathIndexBits` is constrained via `Num2Bits`, and the Poseidon inputs are constrained, the overall proof is sound. This is a style concern rather than a soundness bug.

**Impact:** Low — the circuit is sound due to downstream constraints, but the pattern is fragile and could break if refactored.

**Fix:** Use intermediate `signal` variables for clarity and defense-in-depth.

---

### [MEDIUM] FINDING-018: No Rate Limiting on Agent Handshake Endpoint

**File:** `crates/annex-server/src/lib.rs:855-856`
**Attacker:** Rogue Agent, Federation Attacker
**Category:** Resource Exhaustion

**Description:** The `/api/vrp/agent-handshake` endpoint is outside the `protected_routes` group (no auth middleware) and uses the `Default` rate limit category. The rate limiter's default limit may be too generous for an endpoint that performs semantic alignment computation, DB transactions, and reputation scoring.

**Impact:** An attacker can flood the handshake endpoint to exhaust CPU (semantic computation) and database connections.

**Fix:** Add a dedicated rate limit category for VRP handshakes with a lower limit.

---

## LOW Findings

### [LOW] FINDING-019: Signing Key Persistence Uses Non-Atomic File Write

**File:** `crates/annex-server/src/lib.rs:415`
**Attacker:** Disk Thief (race condition)
**Category:** Cryptographic Key Management

**Description:** `std::fs::write()` is not atomic. If the process crashes mid-write, the key file could be corrupted, leading to a different key on restart and federation identity change.

**Fix:** Write to a temporary file, then atomically rename.

---

### [LOW] FINDING-020: No Maximum Topic Length Validation

**File:** `crates/annex-identity/src/lib.rs:134-136`
**Attacker:** Rogue Agent
**Category:** Input Validation

**Description:** Topic strings are validated for emptiness but not maximum length. An attacker could submit extremely long topic strings, causing excessive memory allocation in SHA-256 computation and database storage.

**Fix:** Add a reasonable maximum topic length (e.g., 256 bytes).

---

### [LOW] FINDING-021: Reputation Score Starts at 0.5 for New Peers

**File:** `crates/annex-vrp/src/reputation.rs:67`
**Attacker:** Rogue Agent
**Category:** VRP Trust Protocol

**Description:** New peers start with a reputation of 0.5 (neutral). This means a brand-new agent with no history has the same reputation as one with a mixed track record. There's no "new entity" discount.

**Fix:** Consider starting new entities lower (e.g., 0.3) and requiring a history of ALIGNED outcomes to reach 0.5.

---

### [LOW] FINDING-022: In-Memory Rate Limiter State Lost on Restart

**File:** `crates/annex-server/src/middleware.rs:122-124`
**Attacker:** Any
**Category:** Resource Management

**Description:** Rate limiter state is entirely in-memory. Server restarts reset all rate limit counters, allowing an attacker to bypass rate limits by triggering (or waiting for) a server restart.

**Fix:** For critical endpoints (registration, verification), consider persisting rate limit state or using a shorter rate limit window.

---

## NOTE Findings

### [NOTE] FINDING-023: `identity.circom` All Inputs Private

**File:** `zk/circuits/identity.circom:22`

The `main` component is declared without `{public [...]}`  annotation, making all inputs (sk, roleCode, nodeId) private and only `commitment` public. This is correct for the identity commitment circuit — the secret key must remain private.

---

### [NOTE] FINDING-024: Good Security Practices Observed

The codebase demonstrates several positive security patterns:
- G1/G2 point validation (on-curve + subgroup) for Groth16 proof parsing
- Parameterized SQL queries throughout (no SQL injection)
- CSP headers with restrictive defaults
- SSRF protection in link preview proxy
- Host header poisoning protection with trusted-proxy check
- Session token HMAC with domain-separated key derivation
- Sliding window rate limiter with cleanup
- File upload size limits and content-type validation
- `rustls-tls` (no OpenSSL dependency)

---

### [NOTE] FINDING-025: Commitment Roundtrip Validation in `commitment.rs`

**File:** `crates/annex-identity/src/commitment.rs:36-49`

The `generate_commitment` function correctly validates that `from_be_bytes_mod_order` did not silently reduce the secret key. This pattern should be applied to the other call sites identified in FINDING-008.

---

### [CRITICAL] FINDING-026: Federation Agreement Persisted on Conflict Alignment

**File:** `crates/annex-federation/src/handshake.rs:72-87`
**Attacker:** Federation Attacker
**Category:** Federation Plane

**Description:** The `process_incoming_handshake` function called `validate_federation_handshake()` and persisted the resulting agreement regardless of the alignment status. When VRP validation returned `Conflict`, a federation agreement was still created in the database with `alignment_status = 'Conflict'` and `transfer_scope = 'NoTransfer'`. Downstream code (RTX relay, message relay) that queries for active agreements could find these Conflict agreements, and the transfer scope check was the only barrier — a logic error or future code change could allow unauthorized data flow through Conflict agreements.

**Impact:** A malicious federation peer with conflicting safety policies could establish a persistent agreement record. If any downstream code path checks `active = 1` without also checking `alignment_status != 'Conflict'`, knowledge transfer could bypass VRP alignment enforcement.

**Fix:** Reject Conflict handshakes before persisting. Return `HandshakeError::AlignmentConflict` error. Only Aligned and Partial handshakes create agreements.

**Verification:** New test `conflict_handshake_rejected_no_agreement_persisted` confirms zero rows in `federation_agreements` after a Conflict handshake.

---

### [HIGH] FINDING-027: Agent Channel Type Not Enforced — Humans Can Join Agent Channels

**File:** `crates/annex-server/src/api_channels.rs:309-407`
**Attacker:** Rogue Agent, Network Observer
**Category:** Communication Plane

**Description:** The `join_channel_handler` validates agent alignment for AI agents (step 3) but does not enforce that `Agent`-type channels are restricted to AI agents only. A human identity can join an `Agent`-type channel by calling `POST /api/channels/{channel_id}/join`.

**Impact:** Human identities gain access to agent-specific channels, potentially bypassing agent-specific policy controls (alignment checks, VRP handshake requirements, transfer scope). Agent channels may contain RTX bundles or agent coordination data not intended for human consumption.

**Fix:** Added channel type enforcement between capability check (step 2) and alignment check (step 3): reject non-agent identities from Agent channels with `403 FORBIDDEN`.

---

### [HIGH] FINDING-028: Federation RTX Relay Does Not Enforce Redacted Topics

**File:** `crates/annex-server/src/api_federation.rs:1049-1186`
**Attacker:** Federation Attacker
**Category:** Federation Plane

**Description:** The `receive_federated_rtx_handler` validates transfer scope (step 2), signature (step 3), and bundle structure (step 4), but does NOT check the bundle's `domain_tags` against the remote peer's `redacted_topics` from their capability contract. The `check_redacted_topics` function exists in `annex-rtx::validation` but was never called in the federation relay path. A federation peer could send RTX bundles containing topics they declared as redacted in their VRP handshake.

**Impact:** Knowledge about topics the remote peer declared off-limits (e.g., "finance", "politics") could be transferred across federation boundaries, violating the VRP capability contract's intent.

**Fix:** Added redacted topics enforcement after transfer scope check: fetch the remote handshake's `capability_contract.redacted_topics` from the agreement and call `check_redacted_topics()` before accepting the bundle.

---

### [CRITICAL] FINDING-029: Trusted Setup Uses Hardcoded Low-Entropy Strings

**File:** `zk/scripts/setup-groth16.js:29,45`
**Attacker:** Any (proof forgery)
**Category:** ZK Infrastructure

**Description:** The Powers of Tau ceremony and Phase 2 contribution used hardcoded strings `"random text"` and `"more entropy"` as ceremony entropy (`-e` flag to snarkjs). These are predictable constants — any adversary who knows these strings can reconstruct the toxic waste and forge valid proofs for arbitrary statements, completely breaking the soundness of the Groth16 system.

**Impact:** If these artifacts were generated and deployed, any party could forge valid ZK proofs — proving false identity commitments, fake Merkle membership, etc. The entire ZK identity system would be compromised.

**Fix:** Replaced hardcoded strings with `crypto.randomBytes(32).toString("hex")` for cryptographically random entropy. Note: a proper multi-party ceremony with independent contributors is still recommended for production.

---

### [HIGH] FINDING-030: ZK Integration Test Accepts Errors as Tamper Rejection

**File:** `crates/annex-identity/tests/zk_integration.rs:58-66`
**Attacker:** N/A (test quality)
**Category:** Test Audit

**Description:** The tampered proof verification test accepted both `Ok(false)` and `Err(...)` as successful tamper detection. This is unsound — an `Err` from the verifier could indicate a bug in the verification pipeline (e.g., malformed inputs, wrong key), not necessarily tamper detection. A real attack could cause a different kind of error that gets silently accepted as "fine."

**Impact:** False confidence in tamper detection — the test would pass even if the verifier is broken in a way that produces errors instead of proper `Ok(false)` rejections.

**Fix:** Changed to `expect("tampered proof verification should return Ok, not Err")` followed by `assert!(!valid)`. The verifier must return `Ok(false)` for well-formed but invalid proofs.

---

### [CRITICAL] FINDING-031: Legacy Identities Bypass ZK Proof Requirement

**File:** `crates/annex-server/src/middleware.rs:460-495`
**Attacker:** Rogue Agent, Network Observer
**Category:** Identity Plane

**Description:** When `enforce_zk_proofs` is enabled, the `verify_zk_membership_header` function requires the `x-annex-zk-proof` header but skips the commitment binding check when `expected_commitment_hex` is `None`. This happens for identities that were created before ZK registration was deployed (no commitment in the database). An attacker could submit a valid proof for *any* commitment and gain access to channels, since the binding check (`payload.commitment_hex != expected`) is wrapped in `if let Some(expected)`.

**Impact:** Legacy identities without registered commitments can bypass the proof-to-identity binding, using any valid proof to access protected resources. This undermines the core ZK invariant even when enforcement is explicitly enabled.

**Fix:** When `enforce_zk_proofs` is true and `expected_commitment_hex` is `None`, immediately reject with `FORBIDDEN`. Legacy identities must re-register through the ZK identity flow.

---

### [HIGH] FINDING-032: Commitment Hashes and Merkle Roots Logged in Warnings

**File:** `crates/annex-server/src/middleware.rs:488-493,530-534`
**Attacker:** Disk Thief
**Category:** Error Handling / Information Leakage

**Description:** When ZK proof verification fails (commitment mismatch or root mismatch), the warning logs included both the submitted and expected commitment hex values, and both the submitted and current Merkle root hex values. An attacker with access to log files could use these to reconstruct identity commitments and Merkle tree state for offline analysis.

**Impact:** Log access reveals cryptographic identity material. Commitment hashes could be used for nullifier derivation (see FINDING-003). Root hashes reveal Merkle tree state evolution, enabling offline membership analysis.

**Fix:** Removed commitment hex and root hex values from log messages. Logs now indicate the failure type without exposing cryptographic material.

---

## Re-Audit Findings (Pass 2 — 2026-03-06)

### [CRITICAL] FINDING-033: Federation Handshake Has No Signature Verification

**File:** `crates/annex-server/src/api_federation.rs:597-670`
**Attacker:** Federation Attacker
**Category:** Federation Plane

**Description:** The `federation_handshake_handler` accepted a `HandshakeRequest` containing a `base_url` and VRP handshake payload, resolved the remote instance from the database, and processed the handshake — all without verifying any Ed25519 signature. In contrast, all other federation endpoints (`receive_federated_message_handler`, `join_federated_channel_handler`, `receive_federated_rtx_handler`, `attest_membership_handler`) verified the sender's Ed25519 signature against the instance's registered public key.

This allowed any party who knew a registered instance's `base_url` to forge VRP handshakes, establishing (or modifying) federation agreements without proving control of the instance's private key.

**Impact:** A network observer or man-in-the-middle could forge federation handshakes to:
- Establish federation agreements with crafted policies (Aligned status) to gain data transfer
- Downgrade existing agreements by sending a Conflict handshake
- Inject malicious capability contracts (e.g., removing redacted_topics restrictions)

**Fix:** Added `signature` field to `HandshakeRequest`. The handler now verifies the Ed25519 signature over `base_url\nhandshake_json` using the instance's registered public key before processing the handshake.

**Verification:** Compilation and clippy pass. Signature verification follows the same pattern used by all other federation endpoints.

---

### [HIGH] FINDING-034: WebSocket Broadcast Does Not Re-Verify Channel Membership

**File:** `crates/annex-server/src/api_ws.rs:402-419` (broadcast function)
**Attacker:** Rogue Agent
**Category:** Communication Plane

**Description:** The `ConnectionManager::broadcast()` function delivers messages to all pseudonyms in the `channel_subscriptions` map without re-checking database membership. When a user is removed from a channel (via `leave_channel_handler`, admin action, or identity deactivation), their WebSocket subscription persists until they explicitly unsubscribe or disconnect. During this window, they continue receiving all channel messages.

**Impact:** A kicked or revoked member continues receiving messages on existing WebSocket connections. This violates the channel membership invariant.

**Fix:** This is a known architectural trade-off — re-querying the database on every broadcast would add significant latency. The primary mitigations are:
1. `leave_channel_handler` already calls `unsubscribe()` to remove the WS subscription (verified)
2. FINDING-035 fix ensures channel deletion also clears subscriptions
3. FINDING-036 fix ensures VRP conflict deactivation disconnects the user

Remaining gap: admin-initiated member removal (e.g., kick) should also call `unsubscribe()`. Documented as a known limitation.

---

### [HIGH] FINDING-035: Channel Deletion Does Not Revoke WebSocket Subscriptions

**File:** `crates/annex-server/src/api_channels.rs:233-257`
**Attacker:** Rogue Agent
**Category:** Communication Plane

**Description:** When a moderator deleted a channel via `DELETE /api/channels/{channelId}`, the handler deleted the channel from the database but did NOT notify the `ConnectionManager` to unsubscribe all users. WebSocket subscribers remained in the `channel_subscriptions` map for a non-existent channel, potentially receiving messages if the channel ID was reused or causing stale state.

**Impact:** Stale WebSocket subscriptions for deleted channels. Possible information leakage if channel IDs are reused.

**Fix:** Added `state.connection_manager.unsubscribe_channel(&channel_id).await` after successful channel deletion. Also added `ConnectionManager::unsubscribe_channel()` method that removes all subscribers from a channel and cleans up user subscription maps.

---

### [HIGH] FINDING-036: Identity Deactivation Does Not Terminate WebSocket Connections

**File:** `crates/annex-server/src/api_vrp.rs:233-270`
**Attacker:** Rogue Agent
**Category:** Communication Plane, Identity Plane

**Description:** When an agent's identity was deactivated due to VRP Conflict alignment, the code set `active = 0` in the database and emitted an `AgentDisconnected` observe event, but did NOT disconnect the agent's active WebSocket session. The deactivated agent could continue sending and receiving messages until they reconnected (at which point `auth_middleware` would reject them).

**Impact:** Deactivated agents have a window to continue participating in channels after their VRP alignment is rejected.

**Fix:** Added `state.connection_manager.disconnect_user(&pseudonym_id).await` after VRP Conflict deactivation to immediately terminate the agent's WebSocket session.

---

### [MEDIUM] FINDING-037: No Zeroization of Derived Cryptographic Key Material

**File:** `crates/annex-server/src/api_usernames.rs:37-42`, `crates/annex-server/src/api_ws.rs:37-46`
**Attacker:** Disk Thief (memory dump)
**Category:** Cryptographic Key Management

**Description:** Derived key material (AEAD encryption keys from `derive_aead_key`, WS token HMAC secret from `derive_ws_token_secret`) is stored in plain `[u8; 32]` arrays that are not zeroized on drop. The `zeroize` crate is a transitive dependency (used by `ed25519-dalek`, `chacha20poly1305`, etc.) but is never directly used by ANNEX code. Stack-allocated keys may persist in memory after the function returns; the `ws_token_secret` in `AppState` lives for the entire server lifetime.

**Impact:** A memory dump (core dump, `/proc/mem` access, cold boot attack) could reveal derived cryptographic keys. The AEAD keys are ephemeral per-request but the WS token HMAC secret persists in `AppState`.

**Fix:** Add explicit `zeroize` dependency and implement `Zeroize`/`ZeroizeOnDrop` for key material, or wrap derived keys in `zeroize::Zeroizing<[u8; 32]>`. The WS token secret lifetime is inherently long-lived, so zeroization mainly protects against post-process-exit memory inspection.

---

## Remediation Status

| Finding | Severity | Status |
|---------|----------|--------|
| FINDING-001 | CRITICAL | **FIXED** |
| FINDING-002 | CRITICAL | **FIXED** |
| FINDING-003 | CRITICAL | DOCUMENTED (architectural — requires circuit redesign) |
| FINDING-004 | CRITICAL | **FIXED** |
| FINDING-005 | CRITICAL | **FIXED** |
| FINDING-006 | CRITICAL | **FIXED** |
| FINDING-007 | CRITICAL | DOCUMENTED (infrastructure gap — requires build tooling) |
| FINDING-008 | HIGH | **FIXED** |
| FINDING-009 | HIGH | **FIXED** |
| FINDING-010 | HIGH | **FIXED** |
| FINDING-011 | HIGH | **FIXED** |
| FINDING-012 | HIGH | **FIXED** |
| FINDING-013 | MEDIUM | Documented |
| FINDING-014 | MEDIUM | Documented |
| FINDING-015 | MEDIUM | Documented |
| FINDING-016 | MEDIUM | Documented |
| FINDING-017 | MEDIUM | Documented |
| FINDING-018 | MEDIUM | Documented |
| FINDING-019 | LOW | Documented |
| FINDING-020 | LOW | **FIXED** |
| FINDING-021 | LOW | Documented |
| FINDING-022 | LOW | Documented |
| FINDING-026 | CRITICAL | **FIXED** |
| FINDING-027 | HIGH | **FIXED** |
| FINDING-028 | HIGH | **FIXED** |
| FINDING-029 | CRITICAL | **FIXED** |
| FINDING-030 | HIGH | **FIXED** |
| FINDING-031 | CRITICAL | **FIXED** |
| FINDING-032 | HIGH | **FIXED** |
| FINDING-033 | CRITICAL | **FIXED** |
| FINDING-034 | HIGH | DOCUMENTED (architectural trade-off, mitigated) |
| FINDING-035 | HIGH | **FIXED** |
| FINDING-036 | HIGH | **FIXED** |
| FINDING-037 | MEDIUM | Documented |
| FINDING-038 | CRITICAL | **FIXED** |
| FINDING-039 | HIGH | **FIXED** |
| FINDING-040 | HIGH | **FIXED** |

---

## Re-Audit Findings (Pass 3 — 2026-06-19)

This pass focused on the *packaged* desktop application (deb / AppImage /
NSIS / .app) and a fresh hostile review of the server authorization and
federation surfaces. The desktop bundle had never been installed-and-run in
CI — CI builds the bundle but does not execute it, and the e2e suites drive
the standalone server — so the runtime resource-resolution path below was
unvalidated until now.

### [CRITICAL] FINDING-038: Packaged desktop app cannot locate bundled ZK vkey (and Piper / voices)

**File:** `crates/annex-desktop/src/main.rs` (resource resolution in `main`)
**Attacker:** N/A (availability / correctness; also forces insecure fallback)
**Category:** Desktop packaging, Startup

**Description:** Tauri mangles `bundle.resources` paths that escape the project
dir: `../../zk/keys/membership_vkey.json` lands at runtime under
`<resource_root>/_up_/_up_/zk/keys/membership_vkey.json`, where the resource
root is `<exe_dir>/../lib/Annex` (Linux deb/AppImage), `<exe_dir>` (Windows),
or `<exe_dir>/../Resources` (macOS) — **not** beside the exe. `main.rs` only
probed flat exe-relative paths and a build-time workspace path, so on every
installed bundle the vkey, Piper binary, and voices dir were never found.
With the default `enforce_zk_proofs = true`, `start_embedded_server` then
fails with `StartupError::MissingVerificationKey` — the installed Host-mode
app cannot start its server at all (and with enforcement off it would fall
back to the dummy vkey, i.e. insecure).

**Impact:** The shipped Linux/Windows/macOS desktop app is non-functional in
Host mode out of the box (or silently insecure). Undetected because CI never
installs+runs the bundle.

**Fix:** Added `bundled_resource_paths()` which, for each platform resource
root, forms the `_up_/_up_/…` path the bundler actually emits, and extended
the vkey / Piper / voices candidate lists with it. Verified against the real
`Annex_0.0.1_amd64.deb` and `Annex.AppDir` layouts and covered by a unit test
(`bundled_resource_paths_cover_installed_layouts`).

### [HIGH] FINDING-039: Desktop exit leaks the webrtc-server child and the router public endpoint

**File:** `crates/annex-desktop/src/main.rs`, `webrtc.rs`, `public_endpoint.rs`
**Attacker:** Local (resource exhaustion); Network (stale public exposure)
**Category:** Lifecycle, Resource leak

**Description:** The app spawned `webrtc-server` as a `std::process::Child`
(whose `Drop` does **not** kill the process) and registered a public HTTPS
tunnel via the Annex router, but had no exit handler — `stop_local_webrtc`
was dead code and `release_public_endpoint` was never invoked by the
frontend. On exit the webrtc-server orphaned (holding its 7880–7899 port
across restarts) and the router session stayed advertised until its
server-side timeout, leaving a public URL pointing at a now-dead local server.

**Fix:** `main` now builds the app and runs with a `RunEvent::Exit` handler
that calls shared, idempotent helpers `webrtc::shutdown_local_webrtc` (kills +
reaps the child) and `public_endpoint::release_router_session` (best-effort
blocking `/v1/release`).

### [HIGH] FINDING-040: Last active moderator can be demoted → irreversible admin lockout

**File:** `crates/annex-server/src/api_admin.rs` (`update_member_capabilities_handler`)
**Attacker:** Rogue / compromised moderator
**Category:** Authorization, Availability

**Description:** `PATCH /api/admin/members/{id}/capabilities` checked that the
caller `can_moderate` but applied any capability set unconditionally. A single
moderator could clear `can_moderate` on the founder, every other moderator,
and themselves, leaving the server with zero admins — an irreversible lockout
that also drops the server into the no-moderator self-heal path of
`ensure_founder` (the residual of FINDING-006), where the next identity read
re-promotes the lowest-id active account.

**Fix:** Added `annex_identity::would_remove_last_moderator`, called before the
update; the handler now returns `409 Conflict` when a change would remove the
final active moderator. Demoting non-last moderators (or self, while another
remains) is still allowed. Covered by
`test_would_remove_last_moderator_guards_lockout`.

### Documented residuals (not changed this pass)

- **Federation / RTX peer-URL SSRF (refines FINDING-034 area):** outbound
  federation/RTX requests gate peer `base_url`s with the *string-only*
  `is_url_private_or_reserved`, which never resolves a `Host::Domain`. A peer
  domain that resolves to a private/metadata IP bypasses the check, and
  `federation_http_client()` performs no IP pinning. The link-preview proxy
  already solves this (`api_link_preview::resolve_and_validate` +
  `build_pinned_http_client` with `.resolve()` + per-hop revalidation).
  Recommended fix: route all peer-URL outbound through a shared
  resolve-then-validate-then-pin helper. Deferred here because a correct fix
  is async-resolve-and-pin across ~6 call sites and must avoid making
  DNS-in-unit-tests flaky and regressing the localhost federation test matrix.

- **RTX redacted-topics content scan (refines FINDING-028):**
  `check_redacted_topics` matches only the self-asserted `domain_tags`, not
  `summary` / `reasoning_chain` / `caveats`. A sender can tag a redacted-topic
  bundle as `["general"]` to slip content past the filter. Recommended fix:
  also scan the textual fields (word-boundary, case-insensitive) and fail
  closed, or derive `domain_tags` server-side.

- **`ensure_founder` self-heal runs on an unauthenticated read (residual of
  FINDING-006):** the requester-promotion vector is gone (only the lowest-id
  active identity is promoted), but a privilege-granting write still executes
  in response to an anonymous `GET /api/identity/{id}` when the server has no
  active moderator. FINDING-040 closes the main route into that state.
  Recommended hardening: move the self-heal off the read path (startup +
  authenticated admin actions only) and persist a fixed owner identity rather
  than relying on `MIN(id)`.

---

## Pass 4 — Roadmap vs Reality (2026-06-19)

A deliberate, skeptical sweep asking one question per subsystem: **does the
code do what ROADMAP.md / README.md / the code comments claim?** Every finding
below was confirmed by reading the implementation (file:line). The headline:
the ZK identity primitives, text chat, presence, observability, and the
human↔human WebRTC SFU are genuinely built; but **Voice, RTX security, the ZK
circuit suite, and VRP "semantic"/reputation are marked COMPLETE while major
completion criteria are unmet**, and several docs/comments assert behavior the
code does not implement.

Legend: DELIVERED · PARTIAL · STUB (wired but inert) · FAKE (cannot work) ·
COMMENT-LIE (doc asserts X, code does Y).

### Identity / ZK (Phase 1, 12)

- **P4-ID-1 (HIGH, FAKE/COMMENT-LIE):** README advertised a 5-circuit pipeline;
  only `identity`, `membership`, `membership_v2` exist (`zk/circuits/`,
  `zk/scripts/build-circuits.js`). `channel-eligibility`, `link-pseudonyms`,
  `federation-attestation` do not exist. The "prove capability flags without
  revealing identity" privacy claim has zero ZK backing — channel capability
  gating reads plaintext role flags (`channel_service.rs:466-473`). *(README
  corrected in this pass.)*
- **P4-ID-2 (HIGH, STUB):** The v2 secret-derived nullifier — the fix for the
  disclosed FINDING-003 privacy hole (a v1 topic pseudonym is derivable from
  the public commitment) — is unreachable from any shipped client. The client
  hardcodes `protocolVersion: 'v1'` (`client/src/stores/identity.ts`), ships
  only v1 wasm/zkey, and `enabled_zk_versions` defaults to `["v1"]`. The
  privacy hole is still the only behavior production runs.
- **P4-ID-3 (MEDIUM, PARTIAL):** Per-request ZK proof enforcement
  (`verify_zk_membership_header`) is called from exactly 3 channel routes
  (join, history, voice-join). `send_message`, WS message, RTX publish, and
  admin ride a 1-hour HMAC bearer token minted after a single proof. The
  config comment "channel access endpoints require a valid ZK membership
  proof" overstates a prove-once-then-token model.
- **DELIVERED (verified):** `enforce_zk_proofs` defaults true; dummy-vkey
  refused under enforcement; Merkle restore + root-epoch grace correct; WS
  legacy `?pseudonym=` rejected under enforcement.

### VRP / Agent (Phase 3, 6)

- **P4-VRP-1 (HIGH, STUB/COMMENT-LIE):** Completion criterion "reputation
  scores… affect alignment outcomes" is false. `validate_federation_handshake`
  computes the verdict (`api_vrp.rs:195`) **before** `check_reputation_score`
  is read (`api_vrp.rs:219`); reputation is only stored and used for display
  ordering. *(Module doc corrected this pass.)*
- **P4-VRP-2 (MEDIUM, COMMENT-LIE):** `alignment_score` is hardcoded
  1.0/0.5/0.0 from the discrete status (`annex-vrp/src/lib.rs:207-212`, own
  comment "placeholder for now"); the real cosine value is discarded.
  `types.rs` called the field "computed". *(Doc corrected this pass.)*
- **P4-VRP-3 (MED-HIGH, PARTIAL):** "Semantic alignment" is `BagOfWordsEmbedder`
  TF/cosine word-overlap (`annex-vrp/src/semantic.rs`); no embedding model
  exists (ROADMAP 3.3 unchecked) though the phase is COMPLETE. Paraphrased-but-
  aligned principles misclassify as Conflict. *(Docs corrected this pass.)*
- **P4-VRP-4 (MEDIUM, PARTIAL):** The VRP-negotiated capability contract is
  stored (`agent_registrations.capability_contract_json`) but never enforced
  at action time; channel join checks role flags instead.
- **DELIVERED (verified):** policy-change re-evaluation of active agents/peers
  (`policy.rs` wired at `api_admin.rs:134`) — the ROADMAP 3.5 unchecked box
  under-claims this; agent participant-type + alignment channel gating
  (`channel_service.rs:485-544`).

### Voice (Phase 7)

- **P4-VOICE-1 (MEDIUM, COMMENT-LIE):** There is no LiveKit anywhere; voice is
  a self-built in-process `webrtc`-crate SFU (`annex-voice` dep `webrtc=0.11`).
  Every ROADMAP "LiveKit" line (deployment config, server SDK, room/token) is
  mislabeled. Operators told to "deploy a LiveKit server" have nothing to
  deploy.
- **P4-VOICE-2 (HIGH, FAKE):** Desktop host mode downloads `webrtc-server` from
  `github.com/webrtc/webrtc/releases/...` (`webrtc.rs:238-257`) — a URL that
  404s (Google's source mirror, LiveKit's artifact naming). On every default
  desktop host start `startLocalWebRtc()` runs, the download throws, and the
  catch disables voice (`StartupModeSelector.tsx`). So **voice is silently off
  on a default desktop host install**, even though the in-process SFU would
  work with a loopback override and no download. **FIXED in this pass:**
  `start_local_webrtc` no longer downloads/spawns anything — it enables the
  in-process SFU by setting a loopback `[webrtc]` override (the ~10 KB of dead
  download machinery and its tests were removed). `is_enabled()` only needs a
  non-empty loopback URL; signaling/media flow over the app WebSocket + ICE.
- **P4-VOICE-3 (CRITICAL for the feature, STUB):** Agent voice cannot
  synthesize: no `voice_profiles` are ever loaded into `TtsService`
  (`add_profile` is called only in tests); the handler falls back to a
  `"default"` profile that was never registered → `ProfileNotFound` →
  `"TTS failed"`. The ROADMAP 7.4 integration test
  (`agent_voice_test.rs:137-150`) **asserts the error path** (`assert_eq!(type,
  "error")`, message `contains("TTS failed")`) — the test that supposedly
  proves "audio appears in the room" proves it fails.
- **P4-VOICE-4 (HIGH, PARTIAL):** STT (Whisper) and Bark TTS have real
  subprocess integrations but **no provisioning** (no whisper setup script; no
  Bark wrapper); only Piper has an installer. "Parler-TTS" named in the roadmap
  isn't implemented. The `voice_config_status` endpoint is honest about all of
  this — the one piece that tells the truth.
- **DELIVERED (verified):** human↔human SFU signaling/RTP fan-out is real
  (`annex-voice/src/service.rs`); the join token is a real, well-tested
  HMAC-SHA256 token (`token.rs`) — just not a LiveKit JWT.

### Federation / RTX (Phase 8, 9)

- **P4-FED-1 (HIGH, FAKE/COMMENT-LIE):** RTX bundle **author** signature is
  never verified — on publish it is only length/non-empty checked
  (`annex-rtx/src/validation.rs`, whose own comment admits "does not verify the
  cryptographic signature") then stored; on federated receive only the relay
  hop's envelope signature is checked, not the content. ROADMAP:969 "Receiving
  server validates: bundle signature" is false. The `agent_registrations` table
  has no signing-pubkey column, so the server structurally cannot verify it.
  Any active peer can rewrite third-party bundle content/tags.
- **P4-FED-2 (MED-HIGH, STUB):** RTX is structurally single-hop —
  `receive_federated_rtx` never re-relays, `relay_path` is hardcoded to one
  entry. The "provenance chain / multi-hop / loop prevention" machinery cannot
  engage. ROADMAP 9.4 multi-hop is marked done.
- **P4-FED-3 (HIGH→fixed this pass, was COMMENT-LIE):** Redacted-topics
  enforcement matched only self-asserted `domain_tags`, not content — a sender
  could launder a prohibited topic into prose with a benign tag, contradicting
  criterion 982 "bundles with redacted content are blocked." **Fixed in this
  pass:** `check_redacted_topics` now whole-word-scans `summary`/
  `reasoning_chain`/`caveats` (unit-tested).
- **P4-FED-4 (MEDIUM, STUB):** `expire_stale_agreements` (agreement TTL) is
  exported and tested but has no caller anywhere — no automatic expiration runs
  despite the polished API.
- **P4-FED-5 (MEDIUM, PARTIAL):** RTX federated receive has no freshness/replay
  guard (message/edit/redaction paths do). A captured RTX relay envelope can be
  replayed by an active peer. No federation-to-self/loopback guard on inbound
  paths either.
- **DELIVERED (verified):** message/edit/redaction relay signatures ARE
  verified before persist + broadcast; conflict handshakes are not persisted;
  manual federation revocation IS implemented and proactive policy-change peer
  notification IS implemented — the release notes claiming these are missing
  are **stale in the project's favor**.

### UX / user flow (observed by driving the live UI)

- **P4-UX-1 (MEDIUM):** The proof-generation screen shows a static
  "Loading proof assets…" / "Generating zero-knowledge proof…" label with no
  spinner/progress bar/elapsed time, for an operation the config budgets at
  30–60s. On slower hardware this is indistinguishable from a hang.
- **P4-UX-2 (LOW):** Status signals are inconsistent — the header server chip
  shows a **red** dot while the footer identity shows **green**, and the green
  "Reconnected" banner lingers for several seconds across tab switches rather
  than dismissing promptly.
- **GOOD:** empty states are genuinely well done ("No federation peers" with an
  explanatory line); the Audio/Video settings dialog is clean and honest
  ("No camera detected"); zero console errors through the full happy path; the
  dark theme is polished and consistent. The core text-chat user flow
  (identity → server → proof → channel → message, incl. cold-start persistence)
  makes sense and works end to end.

### Net

Marketing-grade subsystems that are real: text chat, presence, observability,
the ZK membership primitive, human↔human SFU mechanics, the desktop shell. The
overstated ones — **agent voice, RTX cross-server integrity, the broader ZK
circuit/privacy story, and VRP "semantic"/reputation trust** — are where the
roadmap's COMPLETE labels and several comments do not match the code.
