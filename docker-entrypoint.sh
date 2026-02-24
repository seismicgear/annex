#!/bin/sh
set -eu

DATA_DIR="$(dirname "${ANNEX_DB_PATH:-/app/data/annex.db}")"
DB_PATH="${ANNEX_DB_PATH:-/app/data/annex.db}"

# Ensure the data directory exists and is owned by the runtime user.
# The container starts as root so it can fix volume ownership from prior
# runs, then drops to "annex" via gosu before exec-ing the server.
mkdir -p "$DATA_DIR"
chown -R annex:annex "$DATA_DIR"
SLUG="${ANNEX_SERVER_SLUG:-default}"
LABEL="${ANNEX_SERVER_LABEL:-Annex Server}"
DEFAULT_POLICY='{"agent_min_alignment_score":0.8,"agent_required_capabilities":[],"federation_enabled":true,"default_retention_days":30,"voice_enabled":true,"max_members":1000}'
KEY_FILE="$DATA_DIR/signing.key"

# Sanitize slug/label for SQL: escape single quotes by doubling them.
SAFE_SLUG="$(printf '%s' "$SLUG" | sed "s/'/''/g")"
SAFE_LABEL="$(printf '%s' "$LABEL" | sed "s/'/''/g")"

# ── Signing key ──
# If ANNEX_SIGNING_KEY is not set, generate a persistent key on the data
# volume so it survives container restarts.
if [ -z "${ANNEX_SIGNING_KEY:-}" ]; then
    if [ -f "$KEY_FILE" ]; then
        ANNEX_SIGNING_KEY="$(cat "$KEY_FILE")"
        export ANNEX_SIGNING_KEY
    else
        ANNEX_SIGNING_KEY="$(head -c 32 /dev/urandom | od -A n -t x1 | tr -d ' \n')"
        export ANNEX_SIGNING_KEY
        printf '%s' "$ANNEX_SIGNING_KEY" > "$KEY_FILE"
        chmod 600 "$KEY_FILE"
        chown annex:annex "$KEY_FILE"
        echo "Generated signing key at $KEY_FILE"
    fi
fi

# ── Database migrations + seeding ──
# Run the server briefly as the runtime user to trigger migrations.
# Bind to an ephemeral port (port 0) so it doesn't conflict with the
# real server that starts later. Timeout kills it once migrations are done.
MIGRATION_LOG="$(mktemp)"
if ANNEX_HOST=127.0.0.1 ANNEX_PORT=0 ANNEX_LOG_LEVEL=warn \
   timeout 10 gosu annex /app/annex-server > /dev/null 2>"$MIGRATION_LOG"; then
    : # Server ran and exited cleanly
else
    EXIT_CODE=$?
    # Exit code 124 = timeout (expected: server started serving after migrations).
    if [ "$EXIT_CODE" != "124" ] && [ -s "$MIGRATION_LOG" ]; then
        echo "Migration run output:" >&2
        cat "$MIGRATION_LOG" >&2
    fi
fi
rm -f "$MIGRATION_LOG"

# Seed the servers table if it is empty.
COUNT=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM servers;" 2>/dev/null || echo "0")
if [ "$COUNT" = "0" ]; then
    sqlite3 "$DB_PATH" "INSERT INTO servers (slug, label, policy_json) VALUES ('$SAFE_SLUG', '$SAFE_LABEL', '$DEFAULT_POLICY');"
    echo "Seeded server: slug='$SLUG', label='$LABEL'"
else
    # Fix any rows that were seeded with empty '{}' policy_json.
    sqlite3 "$DB_PATH" "UPDATE servers SET policy_json = '$DEFAULT_POLICY' WHERE policy_json = '{}';" || true
fi

# ── Automatic public tunnel via cloudflared ──
# Creates a free Cloudflare Quick Tunnel (*.trycloudflare.com) so the
# server is reachable from anywhere without port-forwarding or DNS setup.
# Disable with ANNEX_TUNNEL=false. Skipped if ANNEX_PUBLIC_URL is already set.
TUNNEL_ENABLED="${ANNEX_TUNNEL:-true}"
TUNNEL_ENABLED="$(printf '%s' "$TUNNEL_ENABLED" | tr '[:upper:]' '[:lower:]')"

if [ "$TUNNEL_ENABLED" != "false" ] && [ "$TUNNEL_ENABLED" != "0" ] && [ "$TUNNEL_ENABLED" != "no" ] \
   && [ -z "${ANNEX_PUBLIC_URL:-}" ] \
   && command -v cloudflared >/dev/null 2>&1; then

    ANNEX_PORT="${ANNEX_PORT:-3000}"
    TUNNEL_LOG="$(mktemp)"

    echo "Starting cloudflared tunnel..."
    cloudflared tunnel --url "http://127.0.0.1:${ANNEX_PORT}" \
        --no-autoupdate 2>"$TUNNEL_LOG" &
    TUNNEL_PID=$!

    # Wait up to 30 seconds for the tunnel URL to appear in stderr.
    TUNNEL_URL=""
    TRIES=0
    while [ $TRIES -lt 60 ]; do
        TUNNEL_URL="$(grep -oE 'https://[a-zA-Z0-9_-]+\.trycloudflare\.com' "$TUNNEL_LOG" | head -1)" || true
        if [ -n "$TUNNEL_URL" ]; then
            break
        fi
        sleep 0.5
        TRIES=$((TRIES + 1))
    done

    if [ -n "$TUNNEL_URL" ]; then
        export ANNEX_PUBLIC_URL="$TUNNEL_URL"
        echo ""
        echo "============================================"
        echo "  Public URL: $TUNNEL_URL"
        echo "============================================"
        echo ""
        echo "  Share this URL — anyone in the world can"
        echo "  connect as long as this container is running."
        echo ""
    else
        echo "WARN: cloudflared started but no tunnel URL detected after 30s" >&2
        # Kill the tunnel process if it didn't produce a URL
        kill "$TUNNEL_PID" 2>/dev/null || true
    fi
    rm -f "$TUNNEL_LOG"
fi

exec gosu annex /app/annex-server
