#!/bin/sh
set -e

DATA_DIR="$(dirname "${ANNEX_DB_PATH:-/app/data/annex.db}")"
DB_PATH="${ANNEX_DB_PATH:-/app/data/annex.db}"
SLUG="${ANNEX_SERVER_SLUG:-default}"
LABEL="${ANNEX_SERVER_LABEL:-Annex Server}"
DEFAULT_POLICY='{"agent_min_alignment_score":0.8,"agent_required_capabilities":[],"federation_enabled":true,"default_retention_days":30,"voice_enabled":true,"max_members":1000}'
KEY_FILE="$DATA_DIR/signing.key"

# ── Signing key ──
# If ANNEX_SIGNING_KEY is not set, generate a persistent key on the data
# volume so it survives container restarts.
if [ -z "$ANNEX_SIGNING_KEY" ]; then
    if [ -f "$KEY_FILE" ]; then
        export ANNEX_SIGNING_KEY="$(cat "$KEY_FILE")"
    else
        export ANNEX_SIGNING_KEY="$(head -c 32 /dev/urandom | od -A n -t x1 | tr -d ' \n')"
        echo "$ANNEX_SIGNING_KEY" > "$KEY_FILE"
        chmod 600 "$KEY_FILE"
        echo "Generated signing key at $KEY_FILE"
    fi
fi

# ── Database migrations + seeding ──
# Run the server once to trigger migrations. It will exit with
# NoServerConfigured if the servers table is empty — that is expected.
/app/annex-server 2>/dev/null || true

# Seed the servers table if it is empty.
COUNT=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM servers;" 2>/dev/null || echo "0")
if [ "$COUNT" = "0" ]; then
    sqlite3 "$DB_PATH" "INSERT INTO servers (slug, label, policy_json) VALUES ('$SLUG', '$LABEL', '$DEFAULT_POLICY');"
    echo "Seeded server: slug='$SLUG', label='$LABEL'"
else
    # Fix any rows that were seeded with empty '{}' policy_json.
    sqlite3 "$DB_PATH" "UPDATE servers SET policy_json = '$DEFAULT_POLICY' WHERE policy_json = '{}';"
fi

exec /app/annex-server
