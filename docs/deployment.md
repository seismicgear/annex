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
| `ANNEX_WEBRTC_URL` | (none) | Internal WebRTC media URL (dev sidecar only; native SFU does not require this) |
| `ANNEX_WEBRTC_PUBLIC_URL` | (none) | Public WebRTC URL announced to remote clients |
| `ANNEX_WEBRTC_API_KEY` | (none) | Dev-mode WebRTC API key |
| `ANNEX_WEBRTC_API_SECRET` | (none) | Dev-mode WebRTC API secret |
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

Older versions of this guide and `docker-compose.yml` ran a LiveKit sidecar for voice. The in-tree code in `crates/annex-voice` is a native WebRTC SFU and does not require LiveKit. The Docker Compose file is being updated accordingly; if you operate an older deployment with a LiveKit sidecar, treat that path as legacy and migrate when convenient.

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

If the voice SFU is reachable at a different address from the API, set:

```bash
ANNEX_WEBRTC_PUBLIC_URL=wss://voice.example.com
```

The server uses `ANNEX_PUBLIC_URL` for invite links and federation signatures, and `ANNEX_WEBRTC_PUBLIC_URL` for the WebSocket URL handed to remote voice clients. Both are auto-detected from trusted forwarded headers (`X-Forwarded-Host`, `X-Forwarded-Proto`) when present, so most deployments behind a single proxy need only the first.

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
