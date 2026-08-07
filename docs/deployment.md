# Deployment Guide

Deploy Annex on a clean machine using Docker Compose. No prior knowledge of the codebase required.

## Prerequisites

- Docker Engine 24+ with Compose v2
- 2GB+ RAM (4GB recommended for voice models)
- 1GB+ disk for database and voice model files

## Quick Start

```bash
git clone <repo-url> annex
cd annex
docker compose up -d
```

The server starts at `http://localhost:3000`. The web client is served from the same port.

## Configuration

### Environment Variables

All configuration can be overridden via environment variables. Set them in `docker-compose.yml` under `annex.environment` or in a `.env` file.

Authoritative env-var names live in `crates/annex-server/src/config.rs::load_config`. This table is a subset for deploy operators; consult the README for the full list.

| Variable | Default | Description |
|----------|---------|-------------|
| `ANNEX_HOST` | `127.0.0.1` | Bind address |
| `ANNEX_PORT` | `3000` | HTTP port |
| `ANNEX_DB_PATH` | `annex.db` | SQLite database file path |
| `ANNEX_CONFIG_PATH` | `config.toml` | Config file path |
| `ANNEX_ZK_KEY_PATH` | `zk/keys/membership_vkey.json` | Groth16 verification key |
| `ANNEX_WEBRTC_URL` | `ws://localhost:7880` | **Vestigial value, but do not clear it.** Nothing dials this address — the SFU is in-process. It survives only as the on/off gate: `VoiceService::is_enabled()` is false when it is empty, and `join_voice` then refuses every call with `voice_not_configured`. Leave it at the default unless you mean to disable voice. |
| `ANNEX_WEBRTC_PUBLIC_URL` | (none) | Public URL announced to remote voice clients. Overridden at startup by `ANNEX_PUBLIC_URL` / the persisted server URL when either is set — see [Reverse proxy](#reverse-proxy-recommended). |
| `ANNEX_WEBRTC_API_KEY` | `devkey` | Signalling auth key |
| `ANNEX_WEBRTC_API_SECRET` | `secret` | Signalling auth secret. Change it for any deployment reachable off-host. |
| `ANNEX_PUBLIC_URL` | (auto-derived) | Publicly-reachable server URL (for invites, federation) |
| `ANNEX_LOG_LEVEL` | `info` | Log level (trace/debug/info/warn/error) |
| `ANNEX_LOG_JSON` | `false` | JSON log output for log aggregation |
| `ANNEX_DB_MAINTENANCE_ENABLED` | `false` | Run periodic SQLite maintenance (checkpoint/ANALYZE/optional VACUUM) |
| `ANNEX_DB_MAINTENANCE_INTERVAL_HOURS` | `24` | Hours between maintenance sweeps |
| `ANNEX_DB_MAINTENANCE_VACUUM` | `false` | Run `VACUUM` during the maintenance window (off by default; blocks writers) |
| `ANNEX_IDEMPOTENCY_TTL_SECONDS` | `604800` (7 days) | Age past which WS-idempotency ledger rows (`clientRequestId` dedupe) are evicted |
| `ANNEX_CORS_ALLOW_DEV_LOCALHOST` | unset (= build type) | Force the dev-localhost CORS relaxation on/off. Always off under `ANNEX_BUILD_PROFILE=production`/`release` |
| `ANNEX_STORAGE_WARN_FREE_BYTES` | `536870912` (512 MiB) | Free-disk threshold for warning |
| `ANNEX_STORAGE_BLOCK_FREE_BYTES` | `67108864` (64 MiB) | Free-disk threshold below which writes are rejected with HTTP 507 |
| `ANNEX_FEDERATION_FRESHNESS_SECONDS` | `300` | Max age (seconds) of a live federated envelope's `created_at` |
| `ANNEX_FEDERATION_FUTURE_SKEW_SECONDS` | `60` | Max future skew (seconds) of a live federated envelope's `created_at` |
| `ANNEX_FEDERATION_OUTBOX_MAX_ATTEMPTS` | `12` | Max delivery attempts before an outbox row is marked `failed` |
| `ANNEX_FEDERATION_OUTBOX_INTERVAL_SECONDS` | `5` | Outbox worker tick interval |
| `ANNEX_FEDERATION_OUTBOX_PER_PEER_BATCH` | `8` | Max outbox rows drained per peer per tick (fairness cap) |

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
2. STT model (Whisper): place `ggml-base.en.bin` at `ANNEX_STT_MODEL_PATH`.

Without voice models, text channels still work. Voice channels will be unavailable.

## Federation

To federate with another Annex instance:

1. Register the remote instance:
   ```
   POST /federation/handshake
   ```
   With the remote server's VRP anchor snapshot and capability contract.

2. The remote server must also handshake with you (bilateral).

3. Once both servers have `Aligned` or `Partial` status, federation is active.

Federation requires the server to be publicly accessible (not `127.0.0.1`). Set `ANNEX_SERVER_HOST=0.0.0.0` and configure appropriate firewall rules.

## Backup and Restore

### Backup

The SQLite database is the single source of truth. Back it up while the server is running:

```bash
# Using SQLite's built-in backup (safe for WAL mode)
docker compose exec annex sqlite3 /app/data/annex.db ".backup /app/data/backup.db"

# Copy backup out of container
docker compose cp annex:/app/data/backup.db ./backup.db
```

### Restore

```bash
docker compose down
docker compose cp ./backup.db annex:/app/data/annex.db
docker compose up -d
```

## Monitoring

### Health Check

```bash
curl http://localhost:3000/health
# {"status":"ok","version":"0.0.1"}
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

The Tauri desktop app automatically acquires a public endpoint from the Annex router when hosting a server. No manual configuration is needed — the router-provided URL is set as the server's public URL automatically.

## Security Notes

- Run behind a reverse proxy (nginx, Caddy) with TLS for production
- The SQLite database contains message content in plaintext (E2E encryption planned for future)
- ZK verification keys are public (verification is public by design)
- Server signing keys (Ed25519) are generated at startup and stored in the database
- Rate limiting is enabled by default on identity endpoints
