# Content-blind public hosting, version 2

Status: implementation under qualification. Native Windows and macOS acceptance
and the next published release are not complete.

The public URL is `https://www.monolithannex.com/host/<SHA256(host Ed25519 public
key)>`. The application creates the host routing key locally, separate from user
identities. It never uploads that private key. An invite carries this pinned URL.
The router is permitted to observe destination addresses, connection identifiers,
public handshake material, timing, padded packet sizes, and network addresses.
It cannot decrypt application paths, credentials, identity proofs, messages,
uploads, or responses. This is content confidentiality, not traffic anonymity.

Clients connect to `/host/<key hash>/__tunnel`. The host maintains an outbound
authenticated connection to `/api/host/connect`. Ordinary hosted HTTP endpoints
return 426; there is no plaintext compatibility fallback. Both endpoints execute
application actions only after authenticated decryption.

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

Application JSON is UTF-8, limited to 8 MiB per reassembled frame, fragmented into
48 KiB chunks. Plaintext packets consist of a final-chunk byte (0 or 1), a
big-endian uint32 chunk length, chunk bytes, then zero padding to a multiple of
256 bytes. ChaCha20-Poly1305 uses the direction key and a 12-byte nonce containing
four zero bytes and the big-endian uint64 sequence. The initial sequence is zero;
the connection must close before a sequence exceeds uint32 maximum. Additional
authenticated data is SHA-256(transcript) followed by the nonce. The outer packet
contains only sequence and base64(ciphertext plus authentication tag).

Receivers require the next exact sequence, authenticate before reassembly, and
close on replay, missing packets, tampering, invalid padding or size overflow.
The router's queue adapter orders packets from each producer and discards
duplicates before endpoint delivery. It never retries an uncertain send or
replays an application mutation after reconnection.

The fixed public test vectors in `fixtures/public-host-v2.json` were generated
independently with Node's OpenSSL implementation. Rust and browser implementations
must match both directions byte for byte, reject wrong host keys and tampering,
and pass a real cross-process transport test. These are necessary checks, not a
substitute for acceptance using the installed application.

## Vercel transport

The router uses Vercel's built-in Queue service and Runtime Cache, with no Redis
subscription. Queue and function/cache usage remain billable under the existing
Vercel plan. Queue packets contain ciphertext or public handshake material only;
retention is explicitly 60 seconds (the service minimum), not its 24-hour default.
There is no message archive, content index or identity database in the router.

Runtime Cache holds disposable host-to-connection addresses and heartbeats.
Only registration writes an address. Each heartbeat has its own connection key,
so an old disconnect cannot erase a new registration. Cache eviction forces a
fresh connection; it is never treated as authoritative identity storage. The
client's cryptographic key pin remains authoritative. Routing runs in `iad1`;
all packet consumers share that region and consume across deployments. Random,
connection-scoped queue topics prevent a restarted connection from replaying
older retained traffic. All buffers, frame sizes and live streams are bounded.

HTTP uploads and downloads use endpoint-encrypted acknowledgements with at most
32 chunks in flight per operation, each no larger than 64 KiB. The host grants
upload credit in `request_ready`; clients acknowledge every received response
chunk. This bounds outstanding ciphertext without exposing file paths or transfer
semantics to the router. An interrupted write is never automatically repeated.
