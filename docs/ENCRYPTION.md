# Encryption and privacy in v0.1.0

Annex has two distinct message-storage modes. Ordinary channels use encryption
at rest with a server-held key. An explicitly encrypted channel uses client-side
end-to-end encryption (E2EE). Neither mode makes every part of Annex anonymous
or inaccessible to every participant.

## Ordinary channels

Nonempty ordinary message bodies are encrypted with ChaCha20-Poly1305 before
SQLite storage. The key is derived from the server signing key using
HKDF-SHA256 with a separate domain label. The server decrypts on read so that
history, search, moderation, federation, and authorized agents can function.
Malformed marked ciphertext fails authentication; it is not returned as legacy
plaintext. Older unmarked records have a distinct legacy migration path.

**This protects stored message bodies, not against the operator or a compromised
running server.** A database plus its signing key is enough to decrypt those
bodies. Usernames, uploads, logs, and metadata have their own storage paths;
do not describe this as whole-database or whole-disk encryption. Back up the
signing key alongside the database securely; replacing it can make encrypted
history unreadable. See [deployment and recovery](deployment.md#backup-and-restore).

Implementation: `crates/annex-server/src/at_rest.rs`, the message write/read
paths, and `tests/api_at_rest.rs` under the server crate.

## End-to-end encrypted channels

The shipped v2 client implements:

- **Certified devices.** Device certificates bind an encryption key to a
  pseudonym and identity proof, with challenge-bound Groth16 ownership and
  Ed25519 proof of key possession. The client verifies against its pinned
  verifier, not an arbitrary key returned by the server.
- **Signed membership state.** Manifests bind members, certified devices,
  authority, revision, epoch, and channel-key identity. Clients check signatures,
  authority lineage, and stored pins before using the supplied state. Server
  compare-and-swap handling prevents conflicting writes from being silently
  accepted as the same revision.
- **Key epochs.** A random channel content key is wrapped separately to each
  authorized device using X25519, HKDF-SHA256, and ChaCha20-Poly1305. Messages bind
  their ciphertext to channel/authority/epoch context. Clients keep separate
  keyrings per server, identity, and channel and reject rollback/conflicting
  state. Membership changes use the epoch/rekey protocol.
- **Private attachments.** File bytes are encrypted before upload with a random
  file key. Descriptors, keys, names, and content types travel inside the
  authenticated encrypted message. Digests and bounds are checked on download;
  authenticated context binds the file to its channel and epoch. Image metadata
  stripping is implemented for supported image paths, not a universal promise
  that every file format contains no identifying data.
- **Federated authority and transfer.** Authority reconciliation and signed,
  bounded encrypted upload transfer are implemented. Peer transport carries
  ciphertext and validates authority/transfer policy; this is not blanket
  permission to export restricted content.

Core code: `client/src/lib/device-certificate.ts`, `e2e-manifest.ts`,
`e2e-manager-v2.ts`, `e2e-v2.ts`, `encrypted-attachments.ts`, and server services
`channel_manifest.rs`, `e2e_service.rs`, `authority_bootstrap.rs`, and
`federated_upload_service.rs`. Cross-language known-answer tests pin the sealed
box representation in the Rust and TypeScript implementations.

## Trust boundaries that still matter

1. **Endpoints and delivered code.** Malware, a compromised device, browser
   extension, or malicious client build can read plaintext and keys. A web
   client is delivered by its host, so trusting the web origin still matters.
   Download verification and updater signatures do not establish a clean device.
2. **Authority and first contact.** Signatures and pinned lineage protect
   continuity after the trust context is established. They do not prove that a
   newly encountered server is honest or that an admitted member will keep a
   message secret. Review membership, server identity, and unusual trust changes.
3. **Metadata.** Operators and networks can observe connection addresses,
   timing, membership/authorization state, and ciphertext sizes. The server
   stores routing metadata. Pseudonyms and zero-knowledge membership do not
   erase those observations.
4. **History and revocation.** Rotating a key limits subsequent access under the
   protocol. It cannot revoke plaintext or keys a recipient already saved.
   Annex does not claim a per-message Double Ratchet or a formally audited
   forward/post-compromise secrecy guarantee.
5. **Agents and services.** An admitted agent with a recipient key is a plaintext
   recipient. Connecting it to another runtime/provider brings that runtime's
   data handling into your trust decision. Do not claim host blindness for
   content intentionally processed by a host-operated recipient.
6. **Live media.** WebRTC encrypts transport, but the SFU and configured speech
   pipeline remain within the media trust boundary. Channel E2EE for text/files
   is not an end-to-end encryption claim for voice, camera, or screen sharing.
7. **Cryptographic qualification.** The production Groth16 artifacts use a
   single-contributor setup with a public beacon. There is extensive automated
   negative-path testing, but no evidenced independent comprehensive audit or
   multi-party ceremony. Broad multi-device and federated topology acceptance
   remains ongoing.

These limits supersede the older description that v0.1.0 lacked device
certificates, signed manifests, epochs, or encrypted attachments. Those mechanisms
are implemented; their existence does not eliminate the boundaries above.

See [release evidence](RELEASE_READINESS.md), [architecture](architecture.md),
and [the roadmap](../ROADMAP.md) for the distinction between automated coverage,
native acceptance, and work still to qualify.
