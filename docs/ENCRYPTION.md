# Annex Encryption & Privacy Model

Three independent, layered protections. Each is opt-in or transparent and none
breaks AI agents — agents are ordinary members and receive whatever keys a human
member would.

| Layer | Protects against | Server can read? | Status |
|-------|------------------|------------------|--------|
| **E2E channels** | the server, disk thieves, federation peers, anyone | **No** (content-blind) | opt-in per channel |
| **Encryption at rest** | stolen DB file, leaked backups, filesystem access | Yes (it holds the key) | always on, transparent |
| **Metadata hardening** | the signaling relay observing who/when/how-big | n/a (relay was already content-blind) | wire-protocol + primitives |

## 1. End-to-end encrypted channels (content-blind)

Opt-in per channel (moderator toggle, lock indicator in the UI). When enabled,
message bodies are encrypted on the sender's device and only decrypted on
members' devices. The server stores ciphertext and **cannot read it**.

- **Crypto core** — a sealed box (ephemeral X25519 + ECDH + HKDF-SHA256 +
  ChaCha20-Poly1305, wire `epk(32)‖nonce(12)‖ct`). Implemented **byte-identically**
  in Rust (`crates/annex-federation/src/seal.rs::seal_x25519`) and TypeScript
  (`client/src/lib/e2e.ts`), pinned by a frozen cross-language Known-Answer-Test.
  That equivalence is what lets a Rust-side agent and a browser human share one
  key the server never sees.
- **Key distribution (server-blind)** — `crates/annex-server/src/api_e2e.rs` +
  migration 041. The server holds only public X25519 keys (`member_keys`) and
  opaque sealed channel-key blobs (`channel_key_wraps`). A per-channel content
  key (CEK) is sealed to every member's device key. First-write-wins per
  `(channel, recipient, epoch)`; rotation uses a new epoch.
- **Client orchestration** — `client/src/lib/e2e-channel.ts` resolves the CEK by
  adopting the wrap addressed to us or provisioning one; `key-status` prevents
  two members minting rival keys; `reconcile` auto-admits late joiners.
- **Message path** — `client/src/lib/message-crypto.ts` encrypts outgoing /
  decrypts incoming for E2E channels only; it **never** falls back to sending
  plaintext to an E2E channel.

**Agents:** an agent publishes an X25519 key like any member and receives the
CEK sealed to it, so it reads/produces content normally while the server stays
blind.

## 2. Encryption at rest (transparent)

Every non-E2E message body is stored encrypted in SQLite so a stolen database
file or backup is unreadable. The server derives the key from its own Ed25519
signing key (`crates/annex-server/src/at_rest.rs`, HKDF, distinct domain from
username encryption) and decrypts on read — so history, edits, search, agents,
STT, and federation all keep working.

- Encrypt on write (`channel_service::send_message`/`edit_message`,
  `federation_service` receive/edit); decrypt on every read path.
- **Search** can't `LIKE` over ciphertext, so it scans a bounded recent window
  (`annex_channels::scan_messages`), decrypts in memory, and substring-filters
  (documented window trade-off; no plaintext index is kept).
- Decryption is legacy-tolerant: pre-existing plaintext rows and foreign/E2E
  ciphertext pass through untouched.
- E2E channels get this for free on top — their client-ciphertext is itself
  wrapped at rest, and the server still can't read either layer.

This raises the bar against data-at-rest theft; it does **not** hide content
from a compromised live server (that's what layer 1 is for).

## 3. Metadata hardening at the rendezvous

The signaling relay (`monolith-annex/api/signal.js`) was already *content*-blind
(SDP sealed, IPs never exposed). These harden the remaining metadata
(`crates/annex-federation/src/metadata.rs`):

- **Rotating addresses** — peers address a queue by `rendezvous_tag =
  base64url(SHA256(domain ‖ recipient_pub ‖ hourly-bucket))` instead of a stable
  slug. The relay sees opaque tags that rotate hourly, unlinkable across buckets
  and not reversible to a server. The tag is signed, so it can't be re-addressed.
- **Length hiding** — `seal_padded` pads to a fixed 4 KiB block so ciphertext
  length leaks nothing about SDP size.
- **Cover traffic** — `decoy_payload()` is byte-indistinguishable from a real
  payload; posting on a cadence hides *when* real federation happens. By design
  there is no "decoy" flag at the relay — a decoy is just a normal signed
  envelope to a throwaway tag.

The relay supports rotating-tag addressing today (`?tag=`, with `?slug=` legacy);
wiring the (experimental) WebRTC transport to use tags/padding/decoys end-to-end
is the remaining integration.

## At scale (thousands of users)

- E2E key distribution is O(members) sealed blobs per channel, fetched lazily and
  cached per device; convergence avoids rival keys.
- At-rest encryption is per-message AEAD with negligible overhead; only search
  pays a bounded decrypt-scan cost.
- Rotating tags + padding + cover traffic keep the relay from building a social
  graph or timing profile regardless of how many servers federate through it.
