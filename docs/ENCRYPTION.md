# Annex Encryption & Privacy Model

Three independent, layered protections. Each is opt-in or transparent and none
breaks AI agents — agents are ordinary members and receive whatever keys a human
member would.

| Layer | Protects against | Server can read? | Status |
|-------|------------------|------------------|--------|
| **E2E channels** | disk thieves, federation peers, a passive server | **No** (content-blind) — but see the limits box in §1: an ACTIVELY malicious server can still obtain the channel key, and attachments are refused rather than encrypted | opt-in per channel |
| **Encryption at rest** | stolen DB file, leaked backups, filesystem access | Yes (it holds the key) | always on, transparent |
| **Metadata hardening** | the signaling relay observing who/when/how-big | n/a (relay was already content-blind) | wire-protocol + primitives |

## 1. End-to-end encrypted channels (content-blind)

> ### What this does NOT currently defend against
>
> Three limits, stated here rather than further down, because the sentence
> under this box is the one people act on.
>
> **1. A malicious server can obtain the channel key.** Key distribution reads
> a server-provided directory of pseudonyms and X25519 public keys, and a
> legitimate client wraps the channel key to whatever entries it is given. It
> does not authenticate those device keys against an independently verifiable
> member identity, and when it adopts a wrapped key it authenticates that the
> blob was sealed *to it* — which the sealed-box construction guarantees by
> design — not that the sender was entitled to choose and distribute that key.
> A server that substitutes recipient keys in its directory therefore learns
> the CEK, with every primitive behaving exactly as specified. Signing the
> directory with the same server's key would not help.
>
> This is an inference from reading the whole key-distribution path, not an
> exploit that was run. It is what stops the heading's promise from being
> unqualified, and closing it needs client-verifiable identity-to-device-key
> bindings and authenticated channel-key establishment.
>
> **2. Attachments are not encrypted.** The composer encrypts the message and
> uploads the file as ordinary multipart data, so encrypting a message that
> contains a link never encrypted the file behind it. The server now REFUSES
> attachments in an E2EE channel rather than accepting a readable one — the
> honest interim, not the fix. The fix is client-side encryption of the file
> under the channel key.
>
> **3. Device replacement can strand an identity.** The device secret lives in
> a separate IndexedDB store; the server keeps one key per pseudonym, and
> publishing a new one overwrites it. A legitimate identity returning on a
> clean device publishes a replacement key, cannot decrypt the existing wraps,
> and cannot be repaired by ordinary reconciliation because wraps are
> first-write-wins per `(channel, recipient, epoch)`. The epoch machinery is
> also not yet a rotation mechanism: cached keys are returned without checking
> for a newer epoch, the local store holds one key per channel, and message
> bodies carry no key-epoch identifier.
>
> Until 1 and 3 are closed, treat E2EE channels as protecting against disk
> theft, a federation peer, and a passive server — not against a server that
> is actively hostile.

Opt-in per channel (moderator toggle, lock indicator in the UI). When enabled,
message bodies are encrypted on the sender's device and only decrypted on
members' devices. The server stores ciphertext and cannot read it **so long as
it distributes member keys honestly** — see the box above.

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
- **Including the federation outbox.** `federation_service::enqueue_message_envelope`
  and the edit / redaction enqueues store `federation_outbox.envelope_json`
  encrypted. The envelope carries the message body in cleartext, and an outbox
  row outlives the retention sweep that removes the message it was built from —
  so a queue that was never meant to be a second copy of the history was exactly
  that, in plaintext, for as long as delivery took or failed.
  `background::start_federation_outbox_task` decrypts on read, and
  `MessageCipher::decrypt` passes unmarked values through, so rows written before
  this needed no migration.
- Operational consequence, and the same one `release-gates.md` already records
  for `messages.content`: a test or a script that inspects an outbox row has to
  DECRYPT it. `scripts/smoke-federation.sh` does; a `grep` for the message text
  in `federation_outbox` will find nothing and that is the correct behaviour.
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

The WebRTC transport (`crates/annex-federation/src/transport.rs`) now uses this
end-to-end: it addresses peers by their rotating tag (polling its own
current+previous bucket tags), blanks the slugs on the wire, seals **and pads**
every SDP, and keys peers by their Ed25519 public key — so the relay sees only
opaque, constant-size, unlinkable traffic. (The transport remains experimental:
no production caller instantiates it yet, and a wired-in `signal_verifier` must
authorise senders by pubkey since slugs are blank.)

## At scale (thousands of users)

- E2E key distribution is O(members) sealed blobs per channel, fetched lazily and
  cached per device; convergence avoids rival keys.
- At-rest encryption is per-message AEAD with negligible overhead; only search
  pays a bounded decrypt-scan cost. The bound is real and visible to users:
  `GET /api/messages/search` decrypts the most recent `SEARCH_SCAN_CAP` (1000)
  messages per channel in scope and filters in memory, so older matches are
  not found. The response says which happened — `{results, complete,
  scanned_per_channel}` — because an empty `results` with `complete: false`
  means "not in the part we read", not "not here", and the client must not
  render the two the same way.
- Rotating tags + padding + cover traffic keep the relay from building a social
  graph or timing profile regardless of how many servers federate through it.
