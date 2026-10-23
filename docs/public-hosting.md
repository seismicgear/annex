# First-party desktop hosting

Host starts the embedded server, creates or recovers its existing founder,
establishes the public transport, verifies the public route against the local
server identity, saves that address, and creates an authenticated invite before
entering the application. A failed step stays in setup with an actionable error.

Upgrade recovery also covers the old URL setter replacing the server slug with
a hash of its address. Startup restores the configured original slug only when
the existing slug has that exact legacy form and a recorded nullifier derives
the original founder's pseudonym in the configured namespace. Configuration
alone, a later moderator's proof, an unrelated rename, or a conflicting server
cannot authorize the repair. Identity rows and permissions are never reassigned.

The desktop opens an outbound TLS WebSocket to
`https://www.monolithannex.com/api/host/connect` for peer introduction. The public
address is `/host/<sha256-of-Ed25519-public-key>`. A fresh server challenge binds
each connection to that routing key. The private routing key stays beside the
server database; it is separate from user identity keys and survives relaunch.

The current `annex-direct-v3` protocol uses Monolith for one authenticated
handshake and an encrypted SDP offer/answer. Application HTTP, WebSocket and file
traffic travels over direct WebRTC data channels. Each application channel also
performs its own pinned encrypted handshake. The introducer rejects application
frames, and established peers remain alive when its connection ends or renews.
Ordinary hosted HTTP has no plaintext compatibility fallback; the native
acceptance test expects status 410.

STUN discovers peer addresses. No TURN fallback is configured, and both native
and browser implementations reject relayed ICE candidates. A network that cannot
establish a direct peer connection remains unavailable; hosting does not promise
universal NAT or firewall traversal. See [the protocol specification](protocol/public-host.md)
for the threat model, key pinning, framing and implementation limits.

Only Annex API, authorized upload-read, and application WebSocket paths are
forwarded by the authenticated peer to the desktop's fixed loopback listener. Host content cannot replace
the website HTML. Headers, frame sizes, outstanding requests, queues, upload
bytes, and timeouts are bounded. Mutations are not replayed on reconnect. The
desktop reclaims its key-owned address after a connection expires or restarts;
an obsolete lease cannot remove the current connection.

Invites remain `https://monolithannex.com/invite/<payload>` and carry the complete
hosted server address. The share page hands that address and the invite code to
the registered `annex://` handler. A shared hostname is not a server identity:
neither native invite parsing nor client credential renewal strips the host path.

Multipart and large JSON writes use acknowledged chunks over the direct data
channel, outside the introducer's HTTP request body. The
desktop streams those bytes to its existing authenticated API, preserving
authorization, attachment transactions, and deletion behavior. Introduction
does not add an alternate admission or authorization path.

## Qualification

The isolated transport test uses two loopback listeners from the website
repository's `test/fixtures/opaque-native-relay.mjs`. Its broker is explicitly a
test double; the endpoints are the shipping Rust implementations and communicate
through actual sockets. Set `ANNEX_TEST_HOST_RELAY_URL` to
`http://127.0.0.1:30751` and `ANNEX_TEST_HOST_VISITOR_URL` to
`http://127.0.0.1:30752`, then run the ignored
`public_transport::tests::direct_peers_survive_introducer_shutdown_with_http_websockets_and_files`
test. The website fixture must implement the current introduction protocol.
It covers encrypted HTTP, application sockets, file integrity, malformed-client
isolation and new requests after both fixture introduction listeners stop.
`ANNEX_TEST_TRANSFER_MIB` selects 1–5 MiB; the default is 1 MiB.

For the actual deployed services, set both URLs to
`https://www.monolithannex.com` and `ANNEX_TEST_DEPLOYED_ROUTER=1`. The same test
then exercises actual introduction with disposable keys and real native direct
peers. Set `ANNEX_TEST_BROWSER_DIRECT=1` to additionally run the Chromium/native
proof using production browser transport modules, with 1 MiB transfers. That
browser proof blocks further introduction connections after handoff and verifies
continued WebSocket, new HTTP and file-download traffic. The deployed service is
never stopped by this mode. Neither mode manipulates the installed application UI
or counts as native installer acceptance.

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
The host must also record receipt of the Mac's correlation message before that
recipient journey is considered passed. Source-bound Host, recipient, recovery
and installer results are recorded separately in [release readiness](RELEASE_READINESS.md).

## Historical relay qualification

The following records describe the retired content relay, not the current direct
protocol or the 0.1.3 candidate. The deployed Vercel preview passed
opaque handshake and bidirectional packet forwarding and refused plaintext
HTTP. The native loopback test, independent crypto vectors, strict compiler
checks, production client build, 965 client tests and focused flow-control and
asset retry checks passed. Two production transport runs passed on website
source `bfde7113a6524eb2576a17668e8f7ad4aab4174e`, including 5 MiB writes,
idle heartbeats and stable-address restart. Subsequent larger-burst testing
reproduced a queue sequence-gap disconnect; the public router repair and duplex
transfer acceptance remain open. The new encrypted installers are not yet
published or accepted; the installed older candidate is superseded.
The [current release record](RELEASE_READINESS.md) supersedes those publication
and implementation statuses. Do not treat historical transport results as
qualification of later source or installed packages.

Not synchronized to KI AI Congress / Constellation: the existing connector
orientation request failed with SOURCE_READ_TIMEOUT / SQLSTATE 57014, phase
nodes. Preserve this source-linked handoff until that service is available.
