#!/usr/bin/env bash
# backup-restore.test.sh — a restore drill, run automatically.
#
# The reason this exists rather than a note in the deployment guide: the
# migration runner is forward-only with no down migrations, so restoring from
# backup is the ONLY recovery path from a bad upgrade. A recovery path nobody
# has exercised is a recovery path nobody has.
#
# It builds a database, backs it up, destroys the original, restores, and
# asserts the rows came back — including the signing key, which the previous
# four-line backup procedure did not copy at all.
#
# Run: bash scripts/tests/backup-restore.test.sh

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"

BR_OK=0
BR_BAD=0
br_ok()  { echo "[backup-test] OK   $1"; BR_OK=$((BR_OK + 1)); }
br_bad() { echo "[backup-test] FAIL $1" >&2; BR_BAD=$((BR_BAD + 1)); }

if ! command -v sqlite3 >/dev/null 2>&1; then
  # Stated, not silent. `scripts/smoke-federation.sh` needs sqlite3 too and CI
  # installs it; a developer box without it should be told which check it is
  # not getting rather than seeing a green run that skipped one.
  echo "[backup-test] SKIPPED: sqlite3 is not installed, so the restore drill cannot run."
  echo "[backup-test]          Install it (apt-get install sqlite3) to exercise this locally;"
  echo "[backup-test]          CI has it and runs this check."
  exit 0
fi

WORK=$(mktemp -d)
trap 'rm -rf "${WORK}"' EXIT
DATA="${WORK}/data"
OUT="${WORK}/backups"
mkdir -p "${DATA}"

# A database that looks enough like Annex's for the script's checks to mean
# something: a migrations table and a row worth losing.
sqlite3 "${DATA}/annex.db" >/dev/null <<'SQL'
PRAGMA journal_mode=WAL;
CREATE TABLE _annex_migrations (id INTEGER PRIMARY KEY, name TEXT);
INSERT INTO _annex_migrations (name) VALUES ('000_init.sql'), ('001_identity.sql');
CREATE TABLE messages (id INTEGER PRIMARY KEY, content TEXT);
INSERT INTO messages (content) VALUES ('the one message that must survive');
SQL
printf 'deadbeef%.0s' 1 2 3 4 5 6 7 8 > "${DATA}/signing.key"
chmod 600 "${DATA}/signing.key"
mkdir -p "${DATA}/uploads" && printf 'png' > "${DATA}/uploads/a.png"

# ── Backup ─────────────────────────────────────────────────────────────────
if bash "${ROOT_DIR}/scripts/backup.sh" --data-dir "${DATA}" --out "${OUT}" --keep 3 >"${WORK}/backup.log" 2>&1; then
  br_ok "backup.sh wrote and self-verified an archive"
else
  br_bad "backup.sh failed"
  sed 's/^/[backup-test]      /' "${WORK}/backup.log" >&2
fi

ARCHIVE=$(find "${OUT}" -name 'annex-*.tar.gz' | head -1)
if [ -n "${ARCHIVE}" ]; then
  br_ok "archive exists: $(basename "${ARCHIVE}")"
else
  br_bad "no archive produced"
  echo "[backup-test] ${BR_OK} passed, ${BR_BAD} failed"; exit 1
fi

# The archive holds a signing key; it must not be world-readable.
perms=$(stat -c '%a' "${ARCHIVE}" 2>/dev/null || stat -f '%Lp' "${ARCHIVE}")
if [ "${perms}" = "600" ]; then
  br_ok "archive is mode 600 (it contains the signing key)"
else
  br_bad "archive is mode ${perms}, expected 600"
fi

# ── A corrupt archive must NOT verify ──────────────────────────────────────
#
# Without this the drill proves only that a good archive passes, which is the
# half that was never in doubt.
cp "${ARCHIVE}" "${WORK}/corrupt.tar.gz"
printf 'rot' | dd of="${WORK}/corrupt.tar.gz" bs=1 seek=200 conv=notrunc status=none
if bash "${ROOT_DIR}/scripts/backup.sh" --verify "${WORK}/corrupt.tar.gz" >/dev/null 2>&1; then
  br_bad "a corrupted archive verified — the check proves nothing"
else
  br_ok "a corrupted archive is refused"
fi

# ── Destroy and restore ────────────────────────────────────────────────────
rm -rf "${DATA}"
if bash "${ROOT_DIR}/scripts/restore.sh" --archive "${ARCHIVE}" --data-dir "${DATA}" \
     >"${WORK}/restore.log" 2>&1; then
  br_ok "restore.sh completed"
else
  br_bad "restore.sh failed"
  sed 's/^/[backup-test]      /' "${WORK}/restore.log" >&2
fi

got=$(sqlite3 "${DATA}/annex.db" "SELECT content FROM messages;" 2>/dev/null)
if [ "${got}" = "the one message that must survive" ]; then
  br_ok "the row came back"
else
  br_bad "row not restored (got: '${got}')"
fi

migs=$(sqlite3 "${DATA}/annex.db" "SELECT count(*) FROM _annex_migrations;" 2>/dev/null)
if [ "${migs}" = "2" ]; then
  br_ok "migration ledger restored (${migs} rows)"
else
  br_bad "migration ledger not restored (got '${migs}')"
fi

# The whole reason this script backs up more than the database.
if [ -f "${DATA}/signing.key" ]; then
  br_ok "signing.key restored — the server keeps its identity"
else
  br_bad "signing.key NOT restored: the server would come back as a different node"
fi

if [ -f "${DATA}/uploads/a.png" ]; then
  br_ok "uploads restored"
else
  br_bad "uploads not restored"
fi

echo "[backup-test] ${BR_OK} passed, ${BR_BAD} failed"
[ "${BR_BAD}" -eq 0 ] || exit 1
