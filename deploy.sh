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
HOST="0.0.0.0"
PORT=3000
DATA_DIR="./data"
SERVER_LABEL="Annex Server"
SERVER_SLUG="default"
PUBLIC_URL=""
SIGNING_KEY=""
LOG_LEVEL="info"
LOG_JSON="false"
SKIP_BUILD="false"
LIVEKIT_URL=""
LIVEKIT_API_KEY=""
LIVEKIT_API_SECRET=""

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
  --host <addr>             Bind address (default: 0.0.0.0)
  --port <port>             Bind port (default: 3000)
  --data-dir <path>         Persistent data directory (default: ./data)
  --server-label <name>     Server display name (default: "Annex Server")
  --server-slug <slug>      URL-safe server identifier (default: "default")
  --public-url <url>        Public URL for federation (default: http://localhost:<port>)
  --signing-key <hex>       Ed25519 signing key (64-char hex; omit for ephemeral)
  --log-level <level>       Log level: trace|debug|info|warn|error (default: info)
  --log-json                Output structured JSON logs
  --skip-build              Skip cargo build (use existing binary)
  --livekit-url <url>       LiveKit server WebSocket URL (optional)
  --livekit-api-key <key>   LiveKit API key (optional)
  --livekit-api-secret <s>  LiveKit API secret (optional)
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
        --livekit-url)    LIVEKIT_URL="$2"; shift 2 ;;
        --livekit-api-key)    LIVEKIT_API_KEY="$2"; shift 2 ;;
        --livekit-api-secret) LIVEKIT_API_SECRET="$2"; shift 2 ;;
        --help|-h)        usage ;;
        *) fail "Unknown option: $1. Use --help for usage." ;;
    esac
done

[[ -z "$PUBLIC_URL" ]] && PUBLIC_URL="http://localhost:$PORT"

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

    if has docker-compose; then
        COMPOSE="docker-compose"
    else
        COMPOSE="docker compose"
    fi
    ok "Docker found"

    step "Building and starting containers"
    export ANNEX_HOST="$HOST"
    export ANNEX_PORT="$PORT"
    export ANNEX_LOG_LEVEL="$LOG_LEVEL"

    $COMPOSE -f "$SCRIPT_DIR/docker-compose.yml" up -d --build || fail "Docker Compose failed"

    ok "Containers started"
    step "Annex is running at http://localhost:$PORT"
    echo ""
    echo "  Logs:    $COMPOSE logs -f annex"
    echo "  Stop:    $COMPOSE down"
    echo "  Restart: $COMPOSE restart annex"
    echo ""
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

# ZK verification key
VKEY_PATH="$SCRIPT_DIR/zk/keys/membership_vkey.json"
[[ -f "$VKEY_PATH" ]] || fail "ZK verification key not found at $VKEY_PATH"
ok "ZK verification key found"

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

    # Run server briefly to trigger migrations (exits with error — expected)
    echo "   Running migrations..."
    ANNEX_DB_PATH="$DB_PATH" \
    ANNEX_HOST="127.0.0.1" \
    ANNEX_PORT="0" \
    ANNEX_ZK_KEY_PATH="$VKEY_PATH" \
    ANNEX_LOG_LEVEL="warn" \
    "$BINARY" 2>/dev/null || true

    if [[ "$HAS_SQLITE" == "true" ]]; then
        sqlite3 "$DB_PATH" "INSERT OR IGNORE INTO servers (slug, label, policy_json) VALUES ('$SERVER_SLUG', '$SERVER_LABEL', '{}');" \
            || fail "Failed to seed server row"
        ok "Database seeded: slug='$SERVER_SLUG', label='$SERVER_LABEL'"
    else
        warn "Cannot seed database without sqlite3 CLI."
        echo "   Run this manually:"
        echo "   sqlite3 $DB_PATH \"INSERT INTO servers (slug, label, policy_json) VALUES ('$SERVER_SLUG', '$SERVER_LABEL', '{}');\""
        echo ""
        echo "   Or install sqlite3:"
        echo "     Debian/Ubuntu: sudo apt install sqlite3"
        echo "     macOS:         brew install sqlite"
        echo "     Arch:          sudo pacman -S sqlite"
    fi
fi

# ── Signing key ──

if [[ -z "$SIGNING_KEY" ]]; then
    warn "No --signing-key provided. Server will use an ephemeral key."
    echo "   This is fine for development but NOT for production."
    echo "   Generate a permanent key: openssl rand -hex 32"
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

if [[ -n "$LIVEKIT_URL" ]]; then
    cat >> "$CONFIG_PATH" <<TOML

[livekit]
url = "$LIVEKIT_URL"
api_key = "$LIVEKIT_API_KEY"
api_secret = "$LIVEKIT_API_SECRET"
TOML
fi

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

[[ -n "$SIGNING_KEY" ]]  && export ANNEX_SIGNING_KEY="$SIGNING_KEY"
[[ "$LOG_JSON" == "true" ]] && export ANNEX_LOG_JSON="true"

echo ""
echo "  Server starting at http://$HOST:$PORT"
echo "  Public URL: $PUBLIC_URL"
echo "  Data: $DATA_DIR"
echo "  Logs: $LOG_LEVEL"
echo ""
echo "  Press Ctrl+C to stop."
echo ""

exec "$BINARY"
