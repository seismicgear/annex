# First-party desktop hosting

Host starts the embedded server, creates or recovers its existing founder,
establishes the public transport, verifies the public route against the local
server identity, saves that address, and creates an authenticated invite before
entering the application. A failed step stays in setup with an actionable error.

The desktop opens an outbound TLS WebSocket to
`https://www.monolithannex.com/api/host/connect`. There is no router port-forward,
third-party test tunnel, or inbound desktop firewall requirement. The public
address is `/host/<sha256-of-Ed25519-public-key>`. A fresh server challenge binds
each connection to that routing key. The private routing key stays beside the
server database; it is separate from user identity keys and survives relaunch.

The website's `api/host.js` function uses Redis pub/sub to connect independent
Vercel instances. Redis keeps short-lived connection leases and rate counters;
request bodies travel through transient pub/sub rather than stored messages.
Configure `ANNEX_HOST_REDIS_URL` with an authenticated TLS Redis connection.
`REDIS_URL` and `KV_URL` are supported integration aliases. Missing or unavailable
storage fails closed. The gateway requires no website design changes.

Only Annex API, authorized upload-read, and application WebSocket paths are
forwarded to the desktop's fixed loopback listener. Host content cannot replace
the website HTML. Headers, frame sizes, outstanding requests, queues, upload
bytes, and timeouts are bounded. Mutations are not replayed on reconnect. The
desktop reclaims its key-owned address after a connection expires or restarts;
an obsolete lease cannot remove the current connection.

Invites remain `https://monolithannex.com/invite/<payload>` and carry the complete
hosted server address. The share page hands that address and the invite code to
the registered `annex://` handler. A shared hostname is not a server identity:
neither native invite parsing nor client credential renewal strips the host path.

Multipart and large JSON writes use acknowledged WebSocket chunks so the
platform HTTP body cap does not override the application's upload policy. The
desktop streams those bytes to its existing authenticated API, preserving
authorization, attachment transactions, and deletion behavior. The gateway
does not add an alternate admission or authorization path.

## Qualification

The isolated transport test requires real Redis plus two separate relay
processes from the website repository's `test/fixtures/native-relay.mjs`.
Run the explicitly ignored native test
`public_transport::tests::real_relay_routes_native_http_websockets_and_reconnects`
with `ANNEX_TEST_HOST_RELAY_URL` and `ANNEX_TEST_HOST_VISITOR_URL` naming the two
loopback relays. It exercises the shipping Rust connector, live HTTP,
WebSockets, a 5 MiB streamed write, idle heartbeats, and stable-address recovery.
It is transport evidence, not installed-app acceptance.

Publication requires live gateway deployment, actual installer Host/founder/
invite acceptance, and the release pipeline's package/signature/provenance
checks. Native media traversal remains a separate acceptance gate; forwarding
signaling is not proof of cross-network audio or video.

The native Host acceptance workflow now checks the visible Server founder
profile, the public summary against the installed application's actual database,
and identity/address persistence after reopening. The `public-invite` scenario
of `native-invite-acceptance.yml` installs an exact-source DMG on a separate Mac,
opens an app-generated invitation from the independently running native Host,
and checks OS launch, explicit admission, recipient privileges, a correlation
message, and warm delivery. Supply only a disposable acceptance invitation and
the server slug observed from the host; do not supply real user invitations.
These are executable acceptance procedures, not passing results. The host must
also record receipt of the Mac's correlation message before delivery is claimed.

As of this batch, product source `ebe69f2d65b76a20d8b7a878b3bd034688caf48c`
has passed the native real-Redis HTTP/WebSocket/streamed-upload/reconnect test
and all 955 client tests. Candidate installer run `38833587102` is not yet a
release: Windows stopped on a crates.io TLS download interruption; the other
desktop builds remain running. Website PR seismicgear/monolith-annex#32 has a
ready preview, but production Redis provisioning and live acceptance are still
pending the owner's Vercel scope sign-in. No 0.1.2 installer acceptance or
publication is asserted by these source changes.

Not synchronized to KI AI Congress / Constellation: the existing connector
orientation request failed with SOURCE_READ_TIMEOUT / SQLSTATE 57014, phase
nodes. Preserve this source-linked handoff until that service is available.
