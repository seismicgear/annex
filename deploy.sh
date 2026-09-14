#!/usr/bin/env bash
#
# deploy.sh — Deploy an Annex server instance from source or Docker.
#
# Usage:
#   ./deploy.sh                           # Docker (easiest)
#   ./deploy.sh --mode source             # Build from source
#   ./deploy.sh --mode source --port 8080 # Custom port
#   ./deploy.sh --help                    # Show all options
#
set -euo pipefail

# ── Defaults ──

MODE="docker"
# Loopback by default.
#
# This was `0.0.0.0`, so `./deploy.sh` with no arguments put a server on every
# interface — and in docker mode it did that against `docker-compose.yml`,
# which is labelled DEV/LOCAL ONLY and sets wildcard CORS and dev-fixture ZK
# keys. The default invocation of the deployment script was the least safe
# thing it could do.
#
# Binding to all interfaces is now something you ask for, and asking prints a
# warning naming what else you need. See --host.
HOST="127.0.0.1"
PORT=3000
DATA_DIR="./data"
SERVER_LABEL="Annex Server"
SERVER_SLUG="default"
PUBLIC_URL=""
SIGNING_KEY=""
LOG_LEVEL="info"
LOG_JSON="false"
SKIP_BUILD="false"
SKIP_CLIENT="false"
# Opt in to the DEV/LOCAL ONLY compose stack (wildcard CORS, dev ZK keys).
DEV_STACK="false"

# Default server policy matching ServerPolicy::default() in Rust
DEFAULT_POLICY='{"agent_min_alignment_score":0.8,"agent_required_capabilities":[],"federation_enabled":true,"default_retention_days":30,"voice_enabled":true,"max_members":1000}'

# ── Helpers ──

step()  { printf '\n\033[36m:: %s\033[0m\n' "$1"; }
ok()    { printf '   \033[32mOK: %s\033[0m\n' "$1"; }
warn()  { printf '   \033[33mWARN: %s\033[0m\n' "$1"; }
fail()  { printf '   \033[31mFAIL: %s\033[0m\n' "$1"; exit 1; }
has()   { command -v "$1" >/dev/null 2>&1; }

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ── Parse args ──

usage() {
    cat <<'USAGE'
Annex Server Deploy

Usage: ./deploy.sh [OPTIONS]

Options:
  --mode <docker|source>    Deployment mode (default: docker)
  --host <addr>             Bind address (default: 127.0.0.1; use 0.0.0.0 only
                            behind a TLS-terminating reverse proxy)
  --port <port>             Bind port (default: 3000)
  --data-dir <path>         Persistent data directory (default: ./data)
  --server-label <name>     Server display name (default: "Annex Server")
  --server-slug <slug>      URL-safe server identifier (default: "default")
  --public-url <url>        Public URL for federation (default: auto-detected from requests)
  --signing-key <hex>       Ed25519 signing key (64-char hex; omit for ephemeral)
  --log-level <level>       Log level: trace|debug|info|warn|error (default: info)
  --log-json                Output structured JSON logs
  --skip-build              Skip cargo build (use existing binary)
  --dev                     Use the DEV/LOCAL ONLY docker stack (wildcard CORS,
                            dev-fixture ZK keys, production gates off)
  --skip-client             Skip client frontend build (use existing dist)
  --help                    Show this help

Examples:
  ./deploy.sh                                          # Docker, all defaults
  ./deploy.sh --mode source                            # Build from source
  ./deploy.sh --mode source --port 8080 --log-json     # Custom port, JSON logs
  ./deploy.sh --mode source --public-url https://annex.example.com --signing-key $(openssl rand -hex 32)
USAGE
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --mode)           MODE="$2"; shift 2 ;;
        --host)           HOST="$2"; shift 2 ;;
        --port)           PORT="$2"; shift 2 ;;
        --data-dir)       DATA_DIR="$2"; shift 2 ;;
        --server-label)   SERVER_LABEL="$2"; shift 2 ;;
        --server-slug)    SERVER_SLUG="$2"; shift 2 ;;
        --public-url)     PUBLIC_URL="$2"; shift 2 ;;
        --signing-key)    SIGNING_KEY="$2"; shift 2 ;;
        --log-level)      LOG_LEVEL="$2"; shift 2 ;;
        --log-json)       LOG_JSON="true"; shift ;;
        --skip-build)     SKIP_BUILD="true"; shift ;;
        --dev)            DEV_STACK="true"; shift ;;
        --skip-client)    SKIP_CLIENT="true"; shift ;;
        --help|-h)        usage ;;
        *) fail "Unknown option: $1. Use --help for usage." ;;
    esac
done

[[ "$MODE" != "docker" && "$MODE" != "source" ]] && fail "Invalid --mode '$MODE'. Must be 'docker' or 'source'."

# Validate port is numeric and in range
if ! [[ "$PORT" =~ ^[0-9]+$ ]] || [[ "$PORT" -lt 1 ]] || [[ "$PORT" -gt 65535 ]]; then
    fail "Invalid --port '$PORT'. Must be a number between 1 and 65535."
fi

# Validate signing key format if provided
if [[ -n "$SIGNING_KEY" ]] && ! [[ "$SIGNING_KEY" =~ ^[0-9a-fA-F]{64}$ ]]; then
    fail "Invalid --signing-key. Must be exactly 64 hex characters (32 bytes)."
fi

mkdir -p "$DATA_DIR"
DATA_DIR="$(cd "$DATA_DIR" && pwd)"
DB_PATH="$DATA_DIR/annex.db"

echo ""
echo "╔══════════════════════════════════════╗"
echo "║         Annex Server Deploy          ║"
echo "╚══════════════════════════════════════╝"
echo ""
echo "  Mode:       $MODE"
echo "  Bind:       $HOST:$PORT"
echo "  Data dir:   $DATA_DIR"
echo "  Public URL: $PUBLIC_URL"
echo "  Log level:  $LOG_LEVEL"
echo ""

# ── Docker mode ──

if [[ "$MODE" == "docker" ]]; then
    step "Checking Docker"
    has docker || fail "Docker not found. Install from https://docker.com"

    if docker compose version >/dev/null 2>&1; then
        COMPOSE="docker compose"
    elif has docker-compose; then
        COMPOSE="docker-compose"
    else
        fail "Neither 'docker compose' (v2) nor 'docker-compose' (v1) found"
    fi
    ok "Docker found"

    step "Building and starting containers"
    export ANNEX_HOST="$HOST"
    export ANNEX_PORT="$PORT"
    export ANNEX_LOG_LEVEL="$LOG_LEVEL"

    # Which compose file, and why it is not always the same one.
    #
    # This always used `docker-compose.yml`, which says DEV/LOCAL ONLY in its
    # own header and sets `ANNEX_BUILD_PROFILE=dev` plus `ANNEX_CORS_ORIGINS=*`.
    # So the default mode of the deployment script deployed the development
    # stack — every production gate off — and nothing said so.
    #
    # `--dev` is now how you ask for that. Otherwise this uses
    # `docker-compose.prod.yml`, which requires a real CORS origin list and a
    # public URL and refuses to start without them.
    if [[ "$DEV_STACK" == "true" ]]; then
        COMPOSE_FILE="$SCRIPT_DIR/docker-compose.yml"
        echo "  Using the DEVELOPMENT stack: wildcard CORS, dev-fixture ZK keys,"
        echo "  every production gate disabled. Do not expose this."
    else
        COMPOSE_FILE="$SCRIPT_DIR/docker-compose.prod.yml"
        if [[ -z "${ANNEX_CORS_ORIGINS:-}" ]]; then
            fail "ANNEX_CORS_ORIGINS is required for a production deployment (e.g. https://annex.example.com).
       The server refuses to start under a production profile without an explicit
       origin list, and a wildcard is forbidden. Pass --dev for the local stack."
        fi
        if [[ -z "${ANNEX_PUBLIC_URL:-}" && -z "$PUBLIC_URL" ]]; then
            fail "ANNEX_PUBLIC_URL (or --public-url) is required for a production deployment."
        fi
        [[ -n "$PUBLIC_URL" ]] && export ANNEX_PUBLIC_URL="$PUBLIC_URL"
    fi

    $COMPOSE -f "$COMPOSE_FILE" up -d --build || fail "Docker Compose failed"

    ok "Containers started"
    step "Annex is running at http://localhost:$PORT"
    echo ""
    echo "  Logs:    $COMPOSE logs -f annex"
    echo "  Stop:    $COMPOSE down"
    echo "  Restart: $COMPOSE restart annex"
    echo ""

    # Auto-open browser on the host (the server inside Docker cannot do this).
    #
    # Defaulted to true unconditionally, which meant deploying to a server over
    # SSH tried to launch a browser on it. Local runs still get the
    # convenience; anything else has to ask.
    if [[ "$HOST" == "127.0.0.1" || "$HOST" == "localhost" ]]; then
        OPEN_ENV="${ANNEX_OPEN_BROWSER:-true}"
    else
        OPEN_ENV="${ANNEX_OPEN_BROWSER:-false}"
    fi
    OPEN_ENV="$(echo "$OPEN_ENV" | tr '[:upper:]' '[:lower:]')"
    if [[ "$OPEN_ENV" != "false" && "$OPEN_ENV" != "0" && "$OPEN_ENV" != "no" ]]; then
        URL="http://localhost:$PORT"
        if has xdg-open; then
            xdg-open "$URL" 2>/dev/null &
        elif has open; then
            open "$URL" 2>/dev/null &
        elif [[ -n "${BROWSER:-}" ]]; then
            "$BROWSER" "$URL" 2>/dev/null &
        fi
    fi

    exit 0
fi

# ── Source mode ──

step "Checking prerequisites"

# Rust
has cargo || fail "Rust not found. Install from https://rustup.rs"
RUST_VER="$(rustc --version | grep -oE '[0-9]+\.[0-9]+')"
ok "Rust $RUST_VER"

# SQLite CLI (optional)
HAS_SQLITE=false
if has sqlite3; then
    HAS_SQLITE=true
    ok "sqlite3 found"
else
    warn "sqlite3 not found (will guide manual seeding)"
fi

# ZK verification keys — bootstrap if missing
VKEY_PATH="$SCRIPT_DIR/zk/keys/membership_vkey.json"
if [[ ! -f "$VKEY_PATH" ]]; then
    step "Bootstrapping ZK circuits and keys"
    has npm || fail "Node.js (npm) is required to build ZK circuits. Install from https://nodejs.org"
    (cd "$SCRIPT_DIR/zk" && npm ci && node scripts/build-circuits.js && node scripts/setup-groth16.js) \
        || fail "ZK circuit build failed"
fi
ok "ZK verification keys verified"

# Piper TTS — bootstrap if missing
PIPER_BIN="$SCRIPT_DIR/assets/piper/piper"
VOICE_MODEL="$SCRIPT_DIR/assets/voices/en_US-lessac-medium.onnx"
if [[ ! -x "$PIPER_BIN" ]] || [[ ! -f "$VOICE_MODEL" ]]; then
    SETUP_PIPER="$SCRIPT_DIR/scripts/setup-piper.sh"
    if [[ -x "$SETUP_PIPER" ]]; then
        step "Bootstrapping Piper TTS voice model"
        "$SETUP_PIPER" || warn "Piper setup failed (voice features will be unavailable)"
    else
        warn "Piper TTS not found — run scripts/setup-piper.sh for voice features"
    fi
else
    ok "Piper TTS binary and voice model present"
fi

# ── Build ──

BINARY="$SCRIPT_DIR/target/release/annex-server"

if [[ "$SKIP_BUILD" == "true" ]]; then
    step "Skipping build (--skip-build)"
    [[ -f "$BINARY" ]] || fail "Binary not found at $BINARY. Remove --skip-build to build."
else
    step "Building annex-server (release)"
    (cd "$SCRIPT_DIR" && cargo build --release --bin annex-server) || fail "Build failed"
    ok "Built $BINARY"
fi

# ── Build client frontend ──

CLIENT_DIR="$SCRIPT_DIR/client/dist"

if [[ "$SKIP_CLIENT" == "true" ]]; then
    step "Skipping client build (--skip-client)"
    if [[ ! -f "$CLIENT_DIR/index.html" ]]; then
        warn "Client dist not found at $CLIENT_DIR. Server will run in API-only mode."
    fi
else
    step "Building client frontend"
    has npm || { warn "Node.js (npm) not found — skipping client build. Server will run in API-only mode."; SKIP_CLIENT="true"; }

    if [[ "$SKIP_CLIENT" != "true" ]]; then
        # Copy ZK artifacts into client public dir for the WASM prover
        ZK_WASM="$SCRIPT_DIR/zk/build/membership_js/membership.wasm"
        ZK_ZKEY="$SCRIPT_DIR/zk/keys/membership_final.zkey"
        if [[ -f "$ZK_WASM" ]] && [[ -f "$ZK_ZKEY" ]]; then
            mkdir -p "$SCRIPT_DIR/client/public/zk"
            cp "$ZK_WASM" "$SCRIPT_DIR/client/public/zk/"
            cp "$ZK_ZKEY" "$SCRIPT_DIR/client/public/zk/"
            ok "ZK artifacts copied to client/public/zk/"
        else
            warn "ZK wasm/zkey not found — client ZK proofs may not work"
        fi

        (cd "$SCRIPT_DIR/client" && npm ci && npm run build) || fail "Client build failed"
        ok "Client built at $CLIENT_DIR"
    fi
fi

# ── Initialize database ──

step "Initializing database at $DB_PATH"

NEEDS_SEED=true
if [[ -f "$DB_PATH" ]] && [[ "$HAS_SQLITE" == "true" ]]; then
    COUNT=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM servers;" 2>/dev/null || echo "0")
    if [[ "$COUNT" -gt 0 ]]; then
        ok "Database already seeded ($COUNT server(s))"
        NEEDS_SEED=false
    fi
fi

if [[ "$NEEDS_SEED" == "true" ]]; then
    if [[ ! -f "$DB_PATH" ]]; then
        echo "   Database will be created on first startup"
    fi

    # Run server briefly to trigger migrations. The server auto-seeds a default
    # server record and then attempts to bind a port. We use timeout to prevent
    # hangs if the server starts serving instead of exiting.
    echo "   Running migrations..."
    MIGRATION_LOG="$(mktemp)"
    if has timeout; then
        ANNEX_DB_PATH="$DB_PATH" \
        ANNEX_HOST="127.0.0.1" \
        ANNEX_PORT="0" \
        ANNEX_ZK_KEY_PATH="$VKEY_PATH" \
        ANNEX_LOG_LEVEL="warn" \
        timeout 30 "$BINARY" >/dev/null 2>"$MIGRATION_LOG" || MIGRATION_STATUS=$?
    else
        ANNEX_DB_PATH="$DB_PATH" \
        ANNEX_HOST="127.0.0.1" \
        ANNEX_PORT="0" \
        ANNEX_ZK_KEY_PATH="$VKEY_PATH" \
        ANNEX_LOG_LEVEL="warn" \
        "$BINARY" >/dev/null 2>"$MIGRATION_LOG" || MIGRATION_STATUS=$?
    fi

    # A failed migration must stop the deploy.
    #
    # This was `|| true` followed by a `warn` that printed the log and carried
    # on. So a malformed migration, an unwritable database path or a corrupt
    # schema produced a warning nobody reads and then a server start against a
    # half-migrated database — and the migration runner is forward-only, so the
    # recovery path from that is restore-from-backup.
    #
    # Exit 124 is `timeout` doing its job: the server ran migrations, seeded,
    # and then sat serving until the timeout killed it. That is the SUCCESS
    # case here and the only non-zero status that is not a failure.
    if [[ "${MIGRATION_STATUS:-0}" -ne 0 && "${MIGRATION_STATUS:-0}" -ne 124 ]]; then
        echo "" >&2
        warn "Migrations failed (exit ${MIGRATION_STATUS}). Server output:"
        cat "$MIGRATION_LOG" >&2
        rm -f "$MIGRATION_LOG"
        fail "Refusing to start a server against a database whose migrations did not complete."
    fi
    if [[ -s "$MIGRATION_LOG" ]]; then
        warn "Migration run produced output (may be expected):"
        head -5 "$MIGRATION_LOG" >&2
    fi
    rm -f "$MIGRATION_LOG"

    if [[ "$HAS_SQLITE" == "true" ]]; then
        # Escape single quotes for SQL (double them)
        SAFE_SLUG="${SERVER_SLUG//\'/\'\'}"
        SAFE_LABEL="${SERVER_LABEL//\'/\'\'}"
        sqlite3 "$DB_PATH" "INSERT OR IGNORE INTO servers (slug, label, policy_json) VALUES ('$SAFE_SLUG', '$SAFE_LABEL', '$DEFAULT_POLICY');" \
            || fail "Failed to seed server row"
        # Fix any previously seeded rows with empty policy_json
        sqlite3 "$DB_PATH" "UPDATE servers SET policy_json = '$DEFAULT_POLICY' WHERE policy_json = '{}';" 2>/dev/null || true
        ok "Database seeded: slug='$SERVER_SLUG', label='$SERVER_LABEL'"
    else
        warn "Cannot seed database without sqlite3 CLI."
        echo "   Run this manually:"
        echo "   sqlite3 $DB_PATH \"INSERT INTO servers (slug, label, policy_json) VALUES ('<slug>', '<label>', '$DEFAULT_POLICY');\""
        echo ""
        echo "   Or install sqlite3:"
        echo "     Debian/Ubuntu: sudo apt install sqlite3"
        echo "     macOS:         brew install sqlite"
        echo "     Arch:          sudo pacman -S sqlite"
    fi
fi

# ── Signing key ──

KEY_FILE="$DATA_DIR/signing.key"

if [[ -z "$SIGNING_KEY" ]]; then
    # Persist a signing key on the data volume so it survives restarts
    if [[ -f "$KEY_FILE" ]]; then
        SIGNING_KEY="$(cat "$KEY_FILE")"
        ok "Loaded signing key from $KEY_FILE"
    elif has openssl; then
        SIGNING_KEY="$(openssl rand -hex 32)"
        printf '%s' "$SIGNING_KEY" > "$KEY_FILE"
        chmod 600 "$KEY_FILE"
        ok "Generated and persisted signing key at $KEY_FILE"
    elif has head; then
        SIGNING_KEY="$(head -c 32 /dev/urandom | od -A n -t x1 | tr -d ' \n')"
        printf '%s' "$SIGNING_KEY" > "$KEY_FILE"
        chmod 600 "$KEY_FILE"
        ok "Generated and persisted signing key at $KEY_FILE"
    else
        warn "No --signing-key provided and cannot generate one."
        echo "   Server will use an ephemeral key (NOT suitable for production)."
        echo "   Generate a permanent key: openssl rand -hex 32"
    fi
fi

# ── Write runtime config ──

CONFIG_PATH="$DATA_DIR/config.toml"
cat > "$CONFIG_PATH" <<TOML
# Generated by deploy.sh on $(date '+%Y-%m-%d %H:%M:%S')

[server]
host = "$HOST"
port = $PORT
public_url = "$PUBLIC_URL"

[database]
path = "$DB_PATH"
busy_timeout_ms = 5000
pool_max_size = 8

[logging]
level = "$LOG_LEVEL"
json = $LOG_JSON
TOML

# The three `--livekit-*` flags that used to be parsed here wrote a `[livekit]`
# section. No config struct has ever read one — the voice SFU is in-process in
# `crates/annex-voice` and the real section is `[webrtc]` — so an operator who
# passed them got a silently ignored configuration and a voice stack that did
# not do what they had just configured. Removed rather than renamed: `[webrtc]`
# has its own env vars and the two are not the same shape.

chmod 600 "$CONFIG_PATH"
ok "Config written to $CONFIG_PATH"

# ── Start server ──

step "Starting Annex server"

export ANNEX_CONFIG_PATH="$CONFIG_PATH"
export ANNEX_ZK_KEY_PATH="$VKEY_PATH"
export ANNEX_DB_PATH="$DB_PATH"
export ANNEX_HOST="$HOST"
export ANNEX_PORT="$PORT"
export ANNEX_LOG_LEVEL="$LOG_LEVEL"
export ANNEX_PUBLIC_URL="$PUBLIC_URL"
export ANNEX_CLIENT_DIR="$CLIENT_DIR"

# Say what this binary is.
#
# `deploy.sh` never set ANNEX_BUILD_PROFILE, and every production gate read it
# directly and passed when it was unset — so a source deployment ran with
# permissive CORS and no ZK-enforcement gate, silently. A release binary now
# defaults to `production` on its own; this makes it explicit and overridable,
# and `--dev` carries through to it.
if [[ "$DEV_STACK" == "true" ]]; then
    export ANNEX_BUILD_PROFILE="${ANNEX_BUILD_PROFILE:-dev}"
else
    export ANNEX_BUILD_PROFILE="${ANNEX_BUILD_PROFILE:-production}"
fi

if [[ "$HOST" != "127.0.0.1" && "$HOST" != "localhost" ]]; then
    echo ""
    echo "  NOTE binding to $HOST — this server is reachable off-host."
    echo "       Terminate TLS in front of it (the server speaks HTTP only),"
    echo "       and set ANNEX_CORS_ORIGINS to your real origin(s). Under a"
    echo "       production profile the server refuses to start without them."
    echo ""
fi

[[ -n "$SIGNING_KEY" ]]  && export ANNEX_SIGNING_KEY="$SIGNING_KEY"
[[ "$LOG_JSON" == "true" ]] && export ANNEX_LOG_JSON="true"

# Auto-open browser for source deployments, but only for a loopback bind —
# opening a browser on a remote host you just deployed to is not a convenience.
if [[ "$HOST" == "127.0.0.1" || "$HOST" == "localhost" ]]; then
    export ANNEX_OPEN_BROWSER="${ANNEX_OPEN_BROWSER:-true}"
else
    export ANNEX_OPEN_BROWSER="${ANNEX_OPEN_BROWSER:-false}"
fi

# TTS/STT paths (if assets are present)
[[ -x "$SCRIPT_DIR/assets/piper/piper" ]] && export ANNEX_TTS_BINARY_PATH="$SCRIPT_DIR/assets/piper/piper"
[[ -d "$SCRIPT_DIR/assets/voices" ]]      && export ANNEX_TTS_VOICES_DIR="$SCRIPT_DIR/assets/voices"

echo ""
echo "  Server starting at http://$HOST:$PORT"
echo "  Public URL: $PUBLIC_URL"
echo "  Data: $DATA_DIR"
echo "  Logs: $LOG_LEVEL"
echo ""
echo "  Press Ctrl+C to stop."
echo ""

exec "$BINARY"
