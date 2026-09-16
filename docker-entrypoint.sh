#!/bin/sh
# Runtime entrypoint. Runs as the non-root `annex` user, needs no Linux
# capabilities, and does not write outside the data volume.
#
# ── What changed and why ──────────────────────────────────────────────────
#
# This used to start as root, `chown -R` the whole data directory, and drop to
# `annex` via `gosu`. `docker-compose.prod.yml` sets `cap_drop: ALL`, and all
# three of those need capabilities it removes — CAP_CHOWN and CAP_FOWNER for
# the recursive chown, CAP_SETUID/CAP_SETGID for gosu. That is a startup
# contract in direct conflict with the hardening beside it, not a missing
# recommendation: the documented production compose file could not run its own
# documented entrypoint.
#
# The repair is not to hand the capabilities back. The image declares
# `USER annex`, Compose names the same uid, and volume ownership is done ONCE
# by a short init service that holds only the capability it needs for exactly
# as long as it needs it. The server process then runs unprivileged for its
# whole life with a read-only root filesystem and no capabilities at all.
#
# Migrations are likewise no longer inferred. The previous version ran the
# entire server under `timeout 10` and treated exit code 124 as proof that
# migrations had succeeded — elapsed time standing in for a result, and a check
# that could only fail if the server exited early, which is the one thing a
# healthy server does not do. `annex-server --migrate` applies migrations and
# exits: 0 means applied, anything else means it did not.

set -eu

DB_PATH="${ANNEX_DB_PATH:-/app/data/annex.db}"
DATA_DIR="$(dirname "$DB_PATH")"

if [ ! -d "$DATA_DIR" ]; then
    echo "FATAL: data directory $DATA_DIR does not exist." >&2
    echo "The volume-init service should have created it. See docker-compose.prod.yml." >&2
    exit 1
fi

# Writability is checked here rather than discovered three steps later as a
# confusing SQLite error. `mktemp` in the directory is the only honest test:
# `[ -w ]` consults permission bits and says nothing about a read-only mount.
if ! probe="$(mktemp "$DATA_DIR/.writable.XXXXXX" 2>/dev/null)"; then
    echo "FATAL: $DATA_DIR is not writable by $(id -un) (uid $(id -u))." >&2
    echo "Check the volume's ownership against the 'user:' in your compose file." >&2
    exit 1
fi
rm -f "$probe"

# ── Migrations ──
# An explicit command with an explicit exit code. A failure stops the container
# rather than falling through to serve traffic against a half-migrated
# database: the migration runner is forward-only, and the recovery path from a
# partial apply is restore-from-backup, which is a bad place to arrive by
# accident.
echo "Applying database migrations..."
if ! /app/annex-server --migrate; then
    echo "FATAL: migrations failed. Refusing to start the server." >&2
    exit 1
fi

# The server row is seeded by `prepare_server` on first start, using
# ANNEX_SERVER_SLUG / ANNEX_SERVER_LABEL. It used to be inserted here with
# `sqlite3` and hand-escaped SQL; two code paths writing the same row is one
# more than necessary, and the escaping was the kind that works until a label
# contains something surprising.

exec /app/annex-server
