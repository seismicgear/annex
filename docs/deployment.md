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

| Variable | Default | Description |
|----------|---------|-------------|
| `ANNEX_SERVER_HOST` | `127.0.0.1` | Bind address |
| `ANNEX_SERVER_PORT` | `3000` | HTTP port |
| `ANNEX_DB_PATH` | `annex.db` | SQLite database file path |
| `ANNEX_CONFIG_PATH` | `config.toml` | Config file path |
| `ANNEX_ZK_KEY_PATH` | `zk/keys/membership_vkey.json` | Groth16 verification key |
| `ANNEX_LIVEKIT_URL` | (none) | LiveKit server WebSocket URL |
| `ANNEX_LIVEKIT_API_KEY` | (none) | LiveKit API key |
| `ANNEX_LIVEKIT_API_SECRET` | (none) | LiveKit API secret |
| `ANNEX_LOG_LEVEL` | `info` | Log level (trace/debug/info/warn/error) |
| `ANNEX_LOG_JSON` | `false` | JSON log output for log aggregation |

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
                    ┌─────────────┐
  Browser ──────────│ Annex Server │──── SQLite (WAL mode)
  (React SPA)       │  (Rust/Axum) │
                    └──────┬──────┘
                           │
                    ┌──────┴──────┐
                    │   LiveKit   │──── WebRTC voice
                    │   Server    │
                    └─────────────┘
```

- **Annex Server**: Handles HTTP API, WebSocket messaging, identity, federation, and observability
- **LiveKit**: WebRTC SFU for voice channels (optional — text works without it)
- **SQLite**: Single-file database with WAL mode for concurrent reads

## Voice Setup

Voice requires LiveKit and voice model files:

1. LiveKit starts automatically via Docker Compose
2. TTS model (Piper): Place `.onnx` voice model files in a mounted volume
3. STT model (Whisper): Place `ggml-base.en.bin` in a mounted volume

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

### Server Summary

```bash
curl http://localhost:3000/api/public/server/summary
```

### Logs

```bash
docker compose logs -f annex
```

With `ANNEX_LOG_JSON=true`, logs are structured JSON suitable for ingestion by Elasticsearch, Loki, or similar.

## Security Notes

- Run behind a reverse proxy (nginx, Caddy) with TLS for production
- The SQLite database contains message content in plaintext (E2E encryption planned for future)
- ZK verification keys are public (verification is public by design)
- Server signing keys (Ed25519) are generated at startup and stored in the database
- Rate limiting is enabled by default on identity endpoints
