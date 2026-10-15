# Configuration reference

Start with [Deployment](deployment.md). Environment names and validation are
defined in `crates/annex-server/src/config.rs`. This table is a subset of settings.
`ANNEX_CLIENT_DIR` is read by the static-file router and defaults to `client/dist`.

## Settings

Configuration still supports explicit overrides, but default operation is autonomous: the node computes identity + presence locally at first boot and persists them in its data directory for deterministic restart behavior.

The server reads configuration from three sources in priority order:

1. **Environment variables** (highest priority)
2. **TOML config file** (`config.toml` or `ANNEX_CONFIG_PATH`)
3. **Built-in defaults** (lowest priority)

The deploy scripts generate a `config.toml` in your data directory. You can also configure everything via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `ANNEX_HOST` | `127.0.0.1` | Bind address |
| `ANNEX_PORT` | `3000` | Bind port |
| `ANNEX_PUBLIC_URL` | *(auto-derived and persisted)* | Advertised presence override; set reachable HTTPS for public invitations. An auto-derived slug does not provision DNS or a tunnel |
| `ANNEX_DB_PATH` | `annex.db` | SQLite database file path |
| `ANNEX_DB_BUSY_TIMEOUT_MS` | `5000` | SQLite busy timeout (1-60000) |
| `ANNEX_DB_POOL_MAX_SIZE` | `8` | Connection pool size (1-64) |
| `ANNEX_LOG_LEVEL` | `info` | Log level or tracing directive |
| `ANNEX_LOG_JSON` | `false` | Structured JSON output |
| `ANNEX_SIGNING_KEY` | *(`{data_dir}/signing.key`, generated and persisted mode 0600 on first boot)* | Ed25519 secret key (hex). Not ephemeral: an ephemeral key rotates on restart and invalidates every session token, voice-join token and federation signature the server ever issued. **Back the file up with the database** — see [Deployment](deployment.md) |
| `ANNEX_ZK_KEY_PATH` | `zk/keys/membership_vkey.json` | v1 Groth16 verification key (loaded only when `enabled_zk_versions` includes `"v1"`) |
| `ANNEX_ZK_KEY_PATH_V2` | `zk/keys/membership_v2_vkey.json` | v2 Groth16 verification key — required under the default posture |
| `ANNEX_EMBEDDING_MODEL_DIR` | `assets/embedding` | Pinned VRP alignment model. A production profile refuses to start without it |
| `ANNEX_CONFIG_PATH` | `config.toml` | Config file path |
| `ANNEX_MERKLE_TREE_DEPTH` | `20` | Merkle tree depth (1-30) |
| `ANNEX_INVITE_BASE_URL` | `https://monolithannex.com/invite` | Base URL for generated invite links |
| `ANNEX_TTS_VOICES_DIR` | `assets/voices` | Piper voice model directory |
| `ANNEX_TTS_BINARY_PATH` | `assets/piper/piper` | Piper binary path |
| `ANNEX_BARK_BINARY_PATH` | `assets/bark/bark_tts.py` | Bark TTS Python wrapper path |
| `ANNEX_STT_MODEL_PATH` | `assets/models/ggml-base.en.bin` | Whisper model path |
| `ANNEX_STT_BINARY_PATH` | `assets/whisper/whisper` | Whisper binary path |
| `ANNEX_CORS_ORIGINS` | *(empty)* | Comma-separated allowed CORS origins |
| `ANNEX_ENFORCE_ZK_PROOFS` | `true` | Require ZK membership proof for channel access |
| `ANNEX_RETENTION_CHECK_INTERVAL_SECONDS` | `3600` | Message retention sweep interval |
| `ANNEX_INACTIVITY_THRESHOLD_SECONDS` | `300` | Presence pruning threshold |
| `ANNEX_PRESENCE_BROADCAST_CAPACITY` | `256` | Broadcast channel buffer (16-10000) |
