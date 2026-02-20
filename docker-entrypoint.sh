#!/bin/sh
set -e

DB_PATH="${ANNEX_DB_PATH:-/app/data/annex.db}"
SLUG="${ANNEX_SERVER_SLUG:-default}"
LABEL="${ANNEX_SERVER_LABEL:-Annex Server}"
DEFAULT_POLICY='{"agent_min_alignment_score":0.8,"agent_required_capabilities":[],"federation_enabled":true,"default_retention_days":30,"voice_enabled":true,"max_members":1000}'

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
