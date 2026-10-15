# Deploy Annex v0.1.1

For an individual desktop installation, use [Installation](installation.md).
This guide is for operators responsible for persistence, HTTPS, admission,
networking, backups, and optional federation or speech services.

## Production Compose

Use Docker Engine with Compose v2. Leave disk capacity for the image, models,
database, uploads, and backups; speech workloads need additional CPU/RAM. There
is no release-bound capacity or supported-concurrency guarantee yet.

```bash
git clone --branch v0.1.1 https://github.com/seismicgear/annex.git
cd annex
export ANNEX_CORS_ORIGINS=https://annex.example.com
export ANNEX_PUBLIC_URL=https://annex.example.com
docker compose -f docker-compose.prod.yml up -d --build
```

Replace the example origin with your actual reachable HTTPS origin and put a TLS
reverse proxy in front of the HTTP listener. This builds the production image from the published tag. The release workflow
also built and pushed a versioned image and ran its startup validator. The ordinary `docker-compose.yml`
is **development-only** (wildcard CORS and development profile); do not use it for
a public deployment. Leave `ANNEX_IMAGE` unset for this source-build path.

**Registry access:** the earlier v0.1.0 release workflow pushed a versioned
container, but an anonymous pull-token request returned HTTP 403 `DENIED` during
this readiness pass. Public v0.1.1 container pull acceptance has not been established
either. Do not rely on a public `docker pull` until package visibility/access is resolved. The source build above and
the public standalone server archive below are the available operator paths.
An authorized registry account may have access; publishing a workflow artifact
does not establish anonymous registry access.

These are operator procedures, not evidence that every hosting provider, clean
OS image, or external network has been accepted. See [the evidence ledger](RELEASE_READINESS.md).

## Standalone Linux server archive

Download `annex-server-0.1.1-x86_64-unknown-linux-gnu.tar.gz` and `SHA256SUMS` from
[the release](https://github.com/seismicgear/annex/releases/tag/v0.1.1). Compare the
archive's checksum, then extract and run from its directory so relative resource
paths resolve:

```bash
sha256sum annex-server-0.1.1-x86_64-unknown-linux-gnu.tar.gz
tar --no-same-owner -xzf annex-server-0.1.1-x86_64-unknown-linux-gnu.tar.gz
cd annex-server-0.1.1-x86_64-unknown-linux-gnu
export ANNEX_HOST=127.0.0.1
export ANNEX_CORS_ORIGINS=https://annex.example.com
export ANNEX_PUBLIC_URL=https://annex.example.com
./annex-server --check
./annex-server
```

`--check` validates startup and exits; it can create/migrate the configured
database and persist a signing key. It is not a read-only inspection command.
The published v0.1.1 archive was downloaded, checksum-verified, and this startup
path exited successfully during the public-readiness pass. Its SHA-256 is
`fd3e14559c51560dbe73fc66b2c97d5ab1d6c6c2b562a3c8d7216190aa0b60d4`.
This is startup validation, not a full remote deployment or capacity test.

The archive includes the backend, configuration, verification keys, and alignment
model. It **does not include the browser client or native speech resources**.
Connect with the desktop client, or [build the matching web client](development.md)
and set `ANNEX_CLIENT_DIR` to its absolute `client/dist` path. Use a service manager
for persistent operation. Keep the database and `signing.key` in durable storage;
back up both, and test restoration. Do not run a public server from a temporary
extraction directory.

## Configuration

### Environment Variables

Supported environment variables override configuration. In Compose, set variables that `docker-compose.prod.yml` explicitly forwards/interpolates, or add the needed variable under the service environment. A `.env` file supplies Compose interpolation; it does not automatically pass every variable into the container.

Authoritative env-var names live in `crates/annex-server/src/config.rs::load_config`. This table is a subset for deploy operators; see [Configuration](configuration.md) and the source for additional settings.

<!-- Keep this table CONTIGUOUS. Prose, or even an HTML comment, between two
     rows ends a GitHub-flavoured Markdown table: the seven
     ANNEX_FEDERATION_* variables used to sit after a paragraph about the
     storage thresholds and rendered as a block of literal pipe characters.
     Explanatory notes go after the last row. -->

| Variable | Default | Description |
|----------|---------|-------------|
| `ANNEX_BUILD_PROFILE` | compiled default (`production` for a release binary, `dev` for a debug build; `annex-desktop` sets `desktop`) | **Governs every production gate.** `production` requires an explicit CORS origin list, forbids a wildcard origin, refuses clustered mode on an in-memory rate limiter, forces the dev-localhost CORS relaxation off, rejects weak or ephemeral signing keys, and refuses to start with `ANNEX_ENFORCE_ZK_PROOFS=false`. `desktop` keeps the artifact and signing-key checks and drops the multi-tenant ones. An unrecognised value falls back to the compiled default rather than to "off". |
| `ANNEX_HOST` | `127.0.0.1` | Bind address |
| `ANNEX_PORT` | `3000` | HTTP port |
| `ANNEX_DB_PATH` | `annex.db` | SQLite database file path |
| `ANNEX_CONFIG_PATH` | `config.toml` | Config file path |
| `ANNEX_ZK_KEY_PATH` | `zk/keys/membership_vkey.json` | Groth16 verification key for the **v1** membership circuit. Only loaded when `security.enabled_zk_versions` includes `"v1"`, which it does not by default |
| `ANNEX_ZK_KEY_PATH_V2` | `zk/keys/membership_v2_vkey.json` | Groth16 verification key for the **v2** membership circuit — the default identity path. With the shipped `enabled_zk_versions = ["v2"]` and `enforce_zk_proofs = true`, a missing or unreadable file here is a hard startup error, so this is an operator-facing requirement rather than an optional override |
| `ANNEX_EMBEDDING_MODEL_DIR` | `assets/embedding` | Directory holding the pinned VRP alignment model (`model.safetensors` + `tokenizer.json`, 7.5 MB, installed by `scripts/setup-embedding-model.sh`). Under a production profile the server **refuses to start** without it: the score it produces decides which peers and agents are trusted, and the lexicon fallback is a different instrument whose verdicts a peer running the pinned model cannot reproduce. Relative to the process working directory; `annex-desktop` resolves it from the bundle |
| `ANNEX_WEBRTC_URL` | `ws://localhost:7880` | **Vestigial value, but do not clear it.** Nothing dials this address — the SFU is in-process. It survives only as the on/off gate: `VoiceService::is_enabled()` is false when it is empty, and `join_voice` then refuses every call with `voice_not_configured`. Leave it at the default unless you mean to disable voice. |
| `ANNEX_WEBRTC_PUBLIC_URL` | (none) | Public URL announced to remote voice clients. Overridden at startup by `ANNEX_PUBLIC_URL` / the persisted server URL when either is set — see [Reverse proxy](#reverse-proxy-recommended). |
| `ANNEX_WEBRTC_API_KEY` | `devkey` | **Inert.** Carried through `VoiceService` to `AgentVoiceClient::connect`, whose parameters are `_api_key` / `_api_secret` — it authenticates nothing. |
| `ANNEX_WEBRTC_API_SECRET` | `secret` | **Inert, and this row used to say "change it for any deployment reachable off-host".** It is not a credential: voice-join authentication is an HMAC over `voice_token_secret`, derived from the server's Ed25519 signing key. Rotating this value protects nothing and telling an operator to rotate it spends their attention on the wrong secret. |
| `ANNEX_PUBLIC_URL` | (auto-derived) | Publicly-reachable server URL (for invites, federation) |
| `ANNEX_LOG_LEVEL` | `info` | Log level (trace/debug/info/warn/error) |
| `ANNEX_LOG_JSON` | `false` | JSON log output for log aggregation |
| `ANNEX_DB_MAINTENANCE_ENABLED` | `false` | Run periodic SQLite maintenance (checkpoint/ANALYZE/optional VACUUM) |
| `ANNEX_DB_MAINTENANCE_INTERVAL_HOURS` | `24` | Hours between maintenance sweeps |
| `ANNEX_DB_MAINTENANCE_VACUUM` | `false` | Run `VACUUM` during the maintenance window (off by default; blocks writers) |
| `ANNEX_IDEMPOTENCY_TTL_SECONDS` | `604800` (7 days) | Age past which WS-idempotency ledger rows (`clientRequestId` dedupe) are evicted |
| `ANNEX_CORS_ALLOW_DEV_LOCALHOST` | unset (= build type) | Force the dev-localhost CORS relaxation on/off. Always off under `ANNEX_BUILD_PROFILE=production`/`release` |
| `ANNEX_STORAGE_MAX_DB_BYTES` | `0` (uncapped) | Size cap for the SQLite database file. **The two thresholds below are headroom beneath this number, so without it neither does anything.** |
| `ANNEX_STORAGE_WARN_FREE_BYTES` | `536870912` (512 MiB) | Headroom beneath the cap at which the server logs a warning. Writes still flow |
| `ANNEX_STORAGE_BLOCK_FREE_BYTES` | `67108864` (64 MiB) | Headroom beneath the cap below which writes are rejected with HTTP 507. Must be smaller than the warning threshold |
| `ANNEX_FEDERATION_FRESHNESS_SECONDS` | `300` | Max age (seconds) of a live federated envelope's `created_at` |
| `ANNEX_FEDERATION_FUTURE_SKEW_SECONDS` | `60` | Max future skew (seconds) of a live federated envelope's `created_at` |
| `ANNEX_FEDERATION_OUTBOX_MAX_ATTEMPTS` | `12` | Max delivery attempts before an outbox row is marked `failed` |
| `ANNEX_FEDERATION_OUTBOX_INTERVAL_SECONDS` | `5` | Outbox worker tick interval |
| `ANNEX_FEDERATION_OUTBOX_PER_PEER_BATCH` | `8` | Max outbox rows drained per peer per tick (fairness cap) |
| `ANNEX_FEDERATION_ALLOW_PRIVATE_PEERS` | `false` | Permit federation peers at private / loopback / link-local addresses — see below |
| `ANNEX_FEDERATION_RELAY_TRANSPORT_ENABLED` | `false` | Starts the experimental relay/rendezvous transport during server startup. Disabled by default; production also requires `ANNEX_SIGNAL_TRUSTED_PEERS`. This is a federation transport, not a public application tunnel. The durable HTTP outbox remains the ordinary delivery path |
| `ANNEX_RTX_MAX_HOPS` | `3` | How far an RTX bundle published by THIS server may travel, in relay hops. Signed into the origin attestation, so it limits this server's own publications rather than what it accepts; a receiver bounds what it accepts with a compile-time ceiling of 5 and takes the minimum. `0` disables relay of locally-published bundles |
| `ANNEX_RTX_REQUIRE_HOP_CHAIN` | `false` | Refuse an RTX envelope that carries no signed hop chain. `false` for one release so peers on older builds keep working — such an envelope is accepted and delivered locally but never re-relayed. Flipping it to `true` is a decision about whether your peers have upgraded, which the software cannot know |

### Storage-threshold notes

These three are not free-*disk* measurements, despite what the two older
names suggest. The server has no portable way to ask the OS how much space
is left (see the `storage_health` module header for why adding `libc` /
`windows_sys` for it was refused), so it measures the database file against
a cap you set. Uncapped is the default: a guessed cap that is too low would
refuse writes on a healthy machine. Leaving it unset means only the reactive
path — SQLite itself returning `SQLITE_FULL` — can close the gate, which is
the late signal the proactive probe exists to get ahead of.

Both thresholds are validated at startup: the blocking threshold must be
smaller than the warning one, and the cap must exceed the blocking
threshold, or the server refuses to start rather than running with a gate
that can never warn or one that closes on an empty database.

#### Federating over a private network

`ANNEX_FEDERATION_ALLOW_PRIVATE_PEERS` defaults to `false`, which refuses any
peer whose `base_url` resolves to a private, loopback or link-local address.
That is the right default for peers reachable over the public internet, but it
makes three ordinary topologies silently undeliverable — every outbox row to
such a peer is dropped at dequeue, with only a log line to say so:

- two servers on the same LAN (`10.*`, `192.168.*`, `172.16-31.*`),
- two containers addressing each other by Compose service name (the name
  resolves to a private IP, and `*.internal` is refused by hostname),
- peers across a VPN — Tailscale's `100.64.0.0/10` is explicitly rejected.

Set `ANNEX_FEDERATION_ALLOW_PRIVATE_PEERS=true` on **both** servers for those
deployments. It relaxes only the private-address rule, and only for federation
peers: malformed and non-`http(s)` peer URLs stay refused, and link previews —
where the URL genuinely comes from untrusted message content — are unaffected.

The check it relaxes is defence-in-depth rather than a trust boundary: nothing
in the server writes an `instances` row, so a peer's `base_url` is only ever
what an operator put there. What you give up is protection against an
`instances` row later edited to point at an internal service. The server logs a
warning at startup whenever the setting is on.

### Config File

`config.toml` provides defaults. Environment variables override config file values.

```toml
[server]
host = "0.0.0.0"
port = 3000

[database]
path = "/app/data/annex.db"
busy_timeout_ms = 5000
pool_max_size = 8

[logging]
level = "info"
json = true
```

## Architecture

```
                    ┌─────────────────────────────┐
  Browser ──────────│ Annex Server (Rust/Axum)    │──── SQLite (WAL mode)
  (React SPA)       │   embedded WebRTC SFU       │
                    │   embedded TTS/STT bridge   │
                    └─────────────────────────────┘
```

- **Annex Server**: HTTP API, WebSocket messaging, identity, federation, observability, and the native WebRTC SFU for voice rooms.
- **SQLite**: Single-file database with WAL mode for concurrent reads.

### Voice transport

Voice is served by a native WebRTC SFU built on `webrtc-rs`, compiled into the
Annex binary (`crates/annex-voice/src/service.rs`). There is no media server to
deploy, no second process to supervise, and no extra port to open for
signalling: offer/answer and ICE candidates ride the app's own `/ws` WebSocket
alongside chat traffic, so anything that already proxies the API also proxies
voice signalling. Media itself is ordinary WebRTC — UDP to the ICE candidates
the server advertises, which is what STUN/TURN configuration is for.

Older versions of this guide and `docker-compose.yml` ran a LiveKit sidecar.
That is gone: `docker-compose.yml` no longer defines the service and the server
never dials an external SFU. `docker-compose.livekit.yml` still exists in the
repo root but nothing includes it — do not use it. If you operate a deployment
that still runs the sidecar, stop it; it has had no traffic since the native SFU
landed.

## Voice Setup

Voice runs inside the Annex process. Provide:

1. TTS model (Piper): place `.onnx` voice model files in `ANNEX_TTS_VOICES_DIR` (or mount a volume there).
2. STT model (Whisper): run `scripts/setup-stt.sh`, which downloads a
   digest-pinned GGML model into `assets/models/` and prints the
   `ANNEX_STT_MODEL_PATH` line to export. `--model tiny.en` is smaller and
   faster; `--verify` checks what is already installed and never downloads.
   Mounting your own model and setting the variable by hand works too.

Human WebRTC voice does not require a speech model. Missing speech resources disable the affected transcription or synthetic-voice provider. Desktop v0.1.0 bundles pinned Piper/voices and native Whisper with `base.en`; the standalone archive does not. The container bundles speech executables and a Piper voice but requires a configured Whisper GGML model. Provider readiness and OS permissions still need checking on the target host.

Speech-to-text is separable from the rest of voice: a call works without it and
simply has no captions. `GET /api/voice/config-status` reports `stt_ready`, and
when that is false, `stt_detail` names which of the four causes it is — model
missing, binary missing, binary present but not executable, or ready — by path.
The client renders that sentence in place of the caption strip.

## Federation

To federate with another Annex instance:

1. Register the remote instance:
   ```
   POST /api/federation/handshake
   ```
   With the remote server's VRP anchor snapshot and capability contract.

2. The remote server must also handshake with you (bilateral).

3. Once both servers have `Aligned` or `Partial` status, federation is active.

Federation requires the server to be publicly accessible (not `127.0.0.1`). Set `ANNEX_HOST=0.0.0.0` and configure appropriate firewall rules.

## Backup and Restore

### Backup

**The database is not the whole deployment.** This section used to say it was,
and told you to copy `annex.db` out of the container. Do that alone and the
restored server comes back with a *different identity*: the Ed25519 signing key
lives at `{data_dir}/signing.key`, not in the database, and every session
token, voice-join token and federation signature the server ever issued is
derived from it. Peers that federated with the old key see an impostor. Uploads
are on disk too.

Use the script, which takes all three and then verifies what it wrote:

```bash
bash scripts/backup.sh --data-dir /app/data --out /backups --keep 14
```

It snapshots the database with SQLite's own `.backup` (consistent against a
*running* server, which a file copy is not under WAL), runs
`PRAGMA integrity_check` on the copy, adds `signing.key` and `uploads/`, writes
a timestamped `tar.gz` at mode 600 — it contains a private key — re-verifies
the archive it just wrote, and prunes to `--keep`.

Schedule it. A cron entry or a compose sidecar is enough; what matters is that
something runs it without being remembered.

```bash
bash scripts/backup.sh --verify /backups/annex-20260914-120000.tar.gz
```

### Restore

The migration runner is **forward-only** — there are no down migrations — so
this is the only recovery path from a bad upgrade. Stop the server first.

```bash
bash scripts/restore.sh --archive /backups/annex-20260914-120000.tar.gz --data-dir /app/data
```

It verifies the archive *before* touching anything, refuses to run over a live
data directory (a `-wal` file usually means a server has the database open),
and moves the existing directory aside rather than overwriting it. Then start
the server and check `GET /readyz`.

`scripts/tests/backup-restore.test.sh` defines the whole drill: build, back up,
reject a corrupt archive, restore, and assert that rows and the signing key return.
Its CI wiring is not an execution receipt. Preserve the actual completed log and
source revision for the release being deployed.

## Monitoring

### Health Check

```bash
curl http://localhost:3000/health
# Expect status "ok" and inspect the version returned by the actual binary.
```

### Event Stream

```bash
# Real-time SSE event stream
curl -N http://localhost:3000/events/stream
```

### Audit-log integrity export

The public event log is hash-chained and Ed25519-signed (ADR-0013). External auditors can export it page by page and verify offline — recompute each row's canonical hash, check the `prev_hash` linkage from `GENESIS`, and verify each `event_signature` over `<signing_domain>\n<event_hash>` using the returned `server_verifying_key`:

```bash
curl "http://localhost:3000/api/public/events/chain?from_seq=1&limit=500"
```

### Server Summary

```bash
curl http://localhost:3000/api/public/server/summary
```

### Logs

```bash
docker compose logs -f annex
```

With `ANNEX_LOG_JSON=true`, logs are structured JSON suitable for ingestion by Elasticsearch, Loki, or similar.

### Storage gate

When SQLite reports disk exhaustion (`SQLITE_FULL` / `SQLITE_IOERR`), or the DB file grows past the configured cap, the server closes its storage gate: mutating HTTP requests are rejected with `507 Insufficient Storage` while reads continue. The gate does not auto-recover (auto-recovery would flap under transient I/O errors). After freeing disk, an operator clears it explicitly — both endpoints require a moderator identity:

```bash
# Inspect the gate (state: healthy | warn | degraded, plus the trip reason)
curl -H "Authorization: Bearer $MOD_TOKEN" http://localhost:3000/api/admin/storage

# Clear it after verifying disk space is available again
curl -X POST -H "Authorization: Bearer $MOD_TOKEN" http://localhost:3000/api/admin/storage/clear
```

The clear endpoint stays reachable while the gate is closed; if the disk is still full, the next failing write simply re-trips the gate.

### Federation outbox

Outbound federated messages are delivered through a durable outbox with bounded retry (see ADR-0008). Rows that exhaust their retry budget are kept with `status=failed` for triage:

```bash
# Queue depth and stuck deliveries (filter: ?status=failed, paginate: ?limit=&offset=)
curl -H "Authorization: Bearer $MOD_TOKEN" http://localhost:3000/api/admin/federation/outbox

# After fixing the peer, return a failed row to the retry rotation
curl -X POST -H "Authorization: Bearer $MOD_TOKEN" http://localhost:3000/api/admin/federation/outbox/42/retry
```

## Public Access

For production deployments, Annex needs to be reachable from the internet for invite links and federation to work.

### Reverse proxy (recommended)

Run behind a reverse proxy (nginx, Caddy) with TLS. Set `ANNEX_PUBLIC_URL` to your public domain:

```bash
ANNEX_PUBLIC_URL=https://annex.example.com
```

That one variable is normally enough for voice too. Because the SFU is
in-process and signals over the app's own WebSocket, **the address a remote
client needs for voice is the address it is already talking to.** At startup the
server takes `ANNEX_PUBLIC_URL` (or, if unset, the public URL persisted in the
`servers` table during zero-config bootstrap) and pushes it into the voice
service, so setting it correctly configures both planes.

`ANNEX_WEBRTC_PUBLIC_URL` remains for the unusual case where voice must be
announced at a different address:

```bash
ANNEX_WEBRTC_PUBLIC_URL=wss://voice.example.com
```

Note the precedence, which is not what the variable name suggests: the value
pushed in at startup **wins over** `ANNEX_WEBRTC_PUBLIC_URL`. So on any server
that has a public URL — which, after first boot, is nearly all of them —
`ANNEX_WEBRTC_PUBLIC_URL` has no effect. Overriding it in practice means an
authenticated `PUT /api/admin/webrtc-public-url` (moderator capability
required), which is also what the desktop host mode uses to push its
router-issued URL into the running server. Note that this is a *different*
route from `PUT /api/admin/public-url`, which updates only the HTTP layer's
public URL and does not touch the voice service.

> **Auto-detection does not cover voice.** The proxy-header fallback
> (`X-Forwarded-Host` / `X-Forwarded-Proto`) fills in the server's public URL on
> the first trusted request, but it writes only to the HTTP layer's state — it
> does not reach the voice service. A deployment that sets nothing and relies on
> forwarded headers will get working invite links and **broken remote voice**:
> the voice service still holds the default `ws://localhost:7880`, which it
> deliberately reports as empty rather than hand a remote client a loopback
> address, so `join_voice` returns `voice_not_configured`. Set
> `ANNEX_PUBLIC_URL` explicitly if anyone will call from off-host.

> Earlier revisions of this page named `ANNEX_LIVEKIT_PUBLIC_URL`. No such variable exists — nothing in the codebase reads it, so a deployment configured from those instructions silently had no SFU URL set at all. The voice settings are the `ANNEX_WEBRTC_*` family (`_URL`, `_PUBLIC_URL`, `_API_KEY`, `_API_SECRET`), matching the `[webrtc]` section of `config.toml`.

`ANNEX_PUBLIC_URL` must be **HTTPS** for invite links to work: the invite format requires it, because the link carries a join secret that must not be readable in transit. On an `http://` public URL the admin panel says so and does not offer the invite action.

### Desktop host mode

Automatic public hosting is an intended desktop capability whose routing service is
not supplied by this checkout. The current desktop client attempts
`POST /v1/register` at `ANNEX_ROUTER_URL` (default
`https://router.annex.net`) with the local listener port, and caches the returned
HTTPS URL/session. A registration response does not establish a reverse tunnel or
make the loopback listener reachable from the internet. No first-party
register/release service or outbound HTTP/WebSocket forwarding connector was found
in the current Annex sources; the default hostname returned DNS NXDOMAIN during
the October 13, 2026 acceptance run.

The companion `monolith-annex` site's `/api/signal` service exchanges signed
federation SDP/ICE envelopes. It does not proxy application HTTP, WebSockets or
media and cannot satisfy this desktop hosting contract. Changing
`ANNEX_ROUTER_URL` to that signaling endpoint will not provision a public host.

Until the actual public-host relay and outbound connector are implemented,
deployed and accepted, desktop host mode can start the local embedded server and
must expose public-endpoint acquisition failure truthfully. A separately
provisioned HTTPS reverse proxy can serve an already reachable Annex deployment;
setting `ANNEX_PUBLIC_URL` only announces its origin and does not create that
network path. Remote voice also requires working WebRTC ICE connectivity, with a
real TURN service for networks that cannot establish a direct media path. Neither
loopback health nor successful signaling alone proves public hosting or remote
media.

## Shared request admission

`ANNEX_RATE_LIMIT_BACKEND=redis` enables atomic admission against a Redis
primary. Set `ANNEX_RATE_LIMIT_REDIS_URL` to its authenticated `redis://` or
certificate-verified `rediss://` endpoint, and set `ANNEX_RATE_LIMIT_NAMESPACE`
to the same nonempty value on every replica of one logical service. Independent
services using the same store need distinct namespaces. Production clustered
mode still refuses the memory backend. Unknown backends are refused in every
profile.

The store owns the clock and performs each sliding-window transition in one
Lua operation. IPs, pseudonyms and endpoint categories have separate, hashed
keys. Replica restarts do not reset a shared budget. Counter keys expire after
two idle windows. Startup exercises scripting permission before opening the
listener; a missing, inaccessible or misconfigured store stops startup. A store
failure during requests returns `503`; quota exhaustion returns `429` with
`Retry-After`. There is no local fallback and no automatic replay of an
uncertain admission. Redis reconnection can restore service once the same
authoritative store is reachable.

Use an ACL account permitted to connect, run scripts, read/write hashes and
expire keys under `annex:admission:v1:*`. Keep credentials in deployment secrets.
Set Redis to `noeviction`; evicting or flushing live keys resets their budgets.
Choose persistence and primary failover guarantees that preserve these keys;
this adapter does not make an asynchronously replicated Redis failover lossless.
It adds shared request admission, not shared SQLite storage, session routing or
media distribution.

The dedicated CI lane runs three actual HTTP server processes against Redis,
including concurrent quota exhaustion, process restart, category isolation,
live scripting-permission withdrawal and recovery. Run it locally with a real
test store:

```bash
ANNEX_TEST_REDIS_URL=redis://127.0.0.1:6379/0 \
  cargo test -p annex-server --test shared_rate_limit_live actual_redis -- --ignored --nocapture
```

The test store must permit creating/removing a temporary ACL user. Every test
uses unique namespaces; it never flushes the store. A missing test endpoint
fails the selected lane. The adapter follows the primary client documentation
for [asynchronous connection management](https://docs.rs/redis/1.7.1/redis/aio/struct.ConnectionManager.html)
and Redis's [atomic rate-limit scripting](https://redis.io/docs/latest/commands/incr/).

## Security Notes

- **Set `ANNEX_BUILD_PROFILE=production`** for anything reachable off-host, unless you are running a release binary with the variable unset — in which case production is already the default. This one variable governs every gate below it: an explicit CORS origin list, the refusal to run clustered mode on an in-memory rate limiter, the dev-localhost CORS relaxation being forced off, weak signing keys being rejected, and ZK enforcement being un-disableable. It used to default to "no gates" when unset, and nothing in these docs or in `deploy.sh` told you to set it. A debug build still defaults to `dev`; the desktop app declares itself `desktop`, which keeps the artifact checks and drops the multi-tenant ones.
- Run behind a reverse proxy (nginx, Caddy) with TLS for production. The server speaks HTTP only.
- **Message content is encrypted at rest.** This said the database holds plaintext and that E2E was "planned for future"; both were false. `crates/annex-server/src/at_rest.rs` wraps non-E2E message bodies with ChaCha20-Poly1305 under a key derived from the server signing key, so a stored row reads `\x01ar1:base64(...)` rather than the message — `scripts/smoke-federation.sh` has to decrypt it rather than string-compare. End-to-end channel keys shipped in migration `041_e2e_channel_keys`. See `docs/ENCRYPTION.md` for the three layers.
- ZK verification keys are public (verification is public by design).
- **`security.enabled_zk_versions` ships as `["v2"]`, and a production profile refuses `"v1"`.** v1's public signals are `[root, commitment]`: nothing in the body is fresh, so a captured `POST /api/zk/verify-membership` request is a bearer credential — replay it after a revocation and the server mints a new session (invariant I-ZK-5). v2 binds a single-use server-issued challenge into the circuit, which is what closes that. There is **no environment-variable override for this field**: an operator who genuinely needs v1 must set it in a config file and run a non-production profile, and should understand that they have re-opened the replay. Consequence for deployment: `ANNEX_ZK_KEY_PATH_V2` must point at a real `membership_v2_vkey.json`, because with the shipped defaults a missing one is a startup failure rather than a degraded mode.
- **Server signing keys (Ed25519) live in a file, not the database**: `{data_dir}/signing.key`, mode `0600`, resolved by `crates/annex-server/src/startup.rs::resolve_signing_key` in the order env var → file → generate-and-persist. Under a production or desktop profile a weak key (all-zero, all-`0xff`, single-byte fill) is rejected and a failure to persist is a startup error rather than a silent ephemeral key — an ephemeral one rotates on restart and invalidates every session token, voice-join token and federation signature the server has ever issued. **Back this file up with the database**; they are a pair.
- Rate limiting is enabled by default on identity endpoints.
