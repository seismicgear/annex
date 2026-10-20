# Direct public hosting, protocol 3

Transport protocol: `annex-direct-v3`. The authenticated encryption format remains
`annex-content-blind-v2`, with separate sessions for introduction and each direct
application channel. See [release readiness](../RELEASE_READINESS.md) for current
source-bound native qualification; a protocol description is not an acceptance
receipt.

The public URL is `https://www.monolithannex.com/host/<SHA256(host Ed25519 public
key)>`. The application creates the host routing key locally, separate from user
identities. It never uploads that private key. An invite carries this pinned URL.
The introducer observes destination addresses, connection identifiers, public
handshake material, introduction timing and network addresses.
It cannot decrypt application paths, credentials, identity proofs, messages,
uploads, or responses. This is content confidentiality, not traffic anonymity.

Clients connect to `/host/<key hash>/__tunnel`. The host maintains an outbound
authenticated connection to `/api/host/connect`, requiring a `ready` response
whose protocol and public URL match exactly. The endpoints exchange a pinned
handshake, then one sealed SDP offer and answer. The introduction socket closes
after handoff. Application traffic uses ordered WebRTC data channels and is
forbidden on the introducer; ordinary hosted HTTP has no plaintext compatibility
fallback.

The Rust and browser implementations configure STUN and reject SDP candidates
with `typ relay`. They do not provide a TURN fallback. Each SDP is limited to
24 KiB, and each description must have exactly `protocol`, `type`, and `sdp`.
The live peer is reused for subsequent channels independently of introduction
renewal or interruption. A failed direct connection is reported as unavailable,
not silently routed through a content relay.

## Endpoint handshake

All concatenations below are bytes; all base64 is canonical RFC 4648 standard.
`D` is UTF-8 `annex-content-blind-v2`. The client generates an ephemeral X25519
key and a fresh 32-byte random nonce. Its 64-byte hello is its public key followed
by its nonce. The host generates a new ephemeral X25519 key for this connection.
The signed transcript is `D || host Ed25519 public key || client hello || host
ephemeral public key`. The 128-byte welcome is the host Ed25519 public key,
host ephemeral public key, then its 64-byte Ed25519 signature of that transcript.

The client verifies the URL's SHA-256 key pin and strict Ed25519 signature before
sending application data. Both endpoints reject noncontributory X25519 keys.
The shared X25519 secret feeds HKDF-SHA256 with SHA-256(transcript) as salt.
Independent 32-byte keys use info `D || /client-to-host` and `D || /host-to-client`.
Session private keys are discarded when the connection closes. A later compromise
of the persistent signing key does not recover earlier session keys.

## Encrypted framing

Application JSON is UTF-8, limited to 8 MiB per reassembled frame. Current senders
fragment it into 32 KiB chunks; receivers accept chunks up to 48 KiB. Plaintext
packets consist of a final-chunk byte (0 or 1), a
big-endian uint32 chunk length, chunk bytes, then zero padding to a multiple of
256 bytes. ChaCha20-Poly1305 uses the direction key and a 12-byte nonce containing
four zero bytes and the big-endian uint64 sequence. The initial sequence is zero;
the connection must close before a sequence exceeds uint32 maximum. Additional
authenticated data is SHA-256(transcript) followed by the nonce. The outer packet
contains only sequence and base64(ciphertext plus authentication tag).

Receivers require the next exact sequence, authenticate before reassembly, and
close on replay, missing packets, tampering, invalid padding or size overflow.
The direct data channel supplies ordered delivery. An uncertain write is not
automatically retried, and application mutations are not replayed after
reconnection.

The fixed public test vectors in `fixtures/public-host-v2.json` were generated
independently with Node's OpenSSL implementation. Rust and browser implementations
must match both directions byte for byte, reject wrong host keys and tampering,
and pass a real cross-process transport test. These are necessary checks, not a
substitute for acceptance using the installed application.

## Direct application transport

`crates/annex-desktop/src/direct_host.rs` owns introduction registrations and
established peers separately. `crates/annex-federation/src/direct_peer.rs` and
`client/src/lib/direct-peer.ts` implement native/browser connections. Direct
application channels repeat the endpoint handshake above with fresh ephemeral
keys before accepting HTTP or WebSocket operations. Introduction session keys
are not reused for application traffic.

The main host bounds pending introductions and peers to 64, expires pending
handshakes after 25 seconds, and limits active application channel handlers to
128. Direct packets are at most 64 KiB. Application endpoints remain subject to
their existing authentication, capability and upload policies. A host route may
only reach the fixed embedded loopback listener and approved application paths;
the introducer and external peers cannot select another proxy origin.

HTTP uploads and downloads use endpoint-encrypted acknowledgements with at most
32 chunks in flight per operation, each no larger than 64 KiB. The host grants
upload credit in `request_ready`; clients acknowledge every received response
chunk. This bounds outstanding ciphertext without exposing file paths or transfer
semantics to the introducer. These packets are carried by the direct connection,
not by Vercel Queue. An interrupted write is never automatically repeated.
