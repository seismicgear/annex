#!/bin/sh
set -e

DB_PATH="${ANNEX_DB_PATH:-/app/data/annex.db}"

# Run the server once to trigger migrations. It will exit with
# NoServerConfigured if the servers table is empty — that is expected.
/app/annex-server 2>/dev/null || true

# Seed the servers table if it is empty.
COUNT=$(sqlite3 "$DB_PATH" "SELECT COUNT(*) FROM servers;" 2>/dev/null || echo "0")
if [ "$COUNT" = "0" ]; then
    SLUG="${ANNEX_SERVER_SLUG:-default}"
    LABEL="${ANNEX_SERVER_LABEL:-Annex Server}"
    sqlite3 "$DB_PATH" "INSERT INTO servers (slug, label, policy_json) VALUES ('$SLUG', '$LABEL', '{}');"
    echo "Seeded server: slug='$SLUG', label='$LABEL'"
fi

exec /app/annex-server
