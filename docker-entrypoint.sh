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

exec gosu annex /app/annex-server
