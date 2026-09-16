#!/usr/bin/env bash
# backup.sh — back up an Annex deployment, verifiably.
#
# The backup story was four lines of documentation telling an operator to run
# `sqlite3 .backup` by hand. Three things were wrong with that, and the third
# is the one that loses a server:
#
#   * Nothing scheduled it, nothing pruned it, and nothing ever checked that a
#     backup could be restored. An unverified backup is a belief, not a backup.
#   * The migration runner is forward-only with no down migrations, so
#     "restore from backup" is the ONLY recovery path from a bad upgrade — and
#     it was the one path with no tooling.
#   * **It backed up the database and not the signing key.** The Ed25519 key
#     lives in `{data_dir}/signing.key`, not in the database (the deployment
#     guide said otherwise for a long time). Restore the database without it
#     and the server comes back with a new identity: every session token,
#     voice-join token and federation signature it ever issued is invalid, and
#     every peer that federated with it now sees an impostor. The two are a
#     pair and this script treats them as one.
#
# Usage:
#   bash scripts/backup.sh --data-dir /app/data --out /backups
#   bash scripts/backup.sh --data-dir /app/data --out /backups --keep 14
#   bash scripts/backup.sh --verify /backups/annex-20260914-120000.tar.gz
#
# Exit codes:
#   0  backup written and verified
#   1  usage / missing input
#   2  the backup was written but did not verify — treat as no backup

set -euo pipefail

DATA_DIR=""
OUT_DIR=""
KEEP=14
VERIFY_ARCHIVE=""

die() { echo "[backup] ERROR $*" >&2; exit 1; }
info() { echo "[backup] $*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --data-dir) DATA_DIR="${2:-}"; shift 2 ;;
    --out)      OUT_DIR="${2:-}"; shift 2 ;;
    --keep)     KEEP="${2:-}"; shift 2 ;;
    --verify)   VERIFY_ARCHIVE="${2:-}"; shift 2 ;;
    -h|--help)
      sed -n '2,30p' "$0" | sed 's/^# \{0,1\}//'
      exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

command -v sqlite3 >/dev/null 2>&1 || die "sqlite3 is required (apt-get install sqlite3)"

# ── Verify mode ────────────────────────────────────────────────────────────
#
# Restores into a temp directory and runs SQLite's own integrity check. This is
# the half that makes the rest mean anything: a backup nobody has restored is a
# backup nobody knows about.
if [ -n "${VERIFY_ARCHIVE}" ]; then
  [ -f "${VERIFY_ARCHIVE}" ] || die "no such archive: ${VERIFY_ARCHIVE}"
  tmp=$(mktemp -d)
  trap 'rm -rf "${tmp}"' EXIT
  tar -xzf "${VERIFY_ARCHIVE}" -C "${tmp}"
  db=$(find "${tmp}" -name 'annex.db' | head -1)
  [ -n "${db}" ] || die "archive contains no annex.db"

  result=$(sqlite3 "${db}" 'PRAGMA integrity_check;' | head -1)
  if [ "${result}" != "ok" ]; then
    echo "[backup] integrity_check said: ${result}" >&2
    exit 2
  fi
  tables=$(sqlite3 "${db}" "SELECT count(*) FROM sqlite_master WHERE type='table';")
  migrations=$(sqlite3 "${db}" "SELECT count(*) FROM _annex_migrations;" 2>/dev/null || echo 0)
  info "integrity ok — ${tables} tables, ${migrations} migrations applied"

  if find "${tmp}" -name 'signing.key' | grep -q .; then
    info "signing.key present"
  else
    echo "[backup] WARNING archive has no signing.key: restoring this gives the" >&2
    echo "[backup]         server a NEW identity and invalidates every token and" >&2
    echo "[backup]         federation signature it has issued." >&2
    exit 2
  fi
  info "VERIFIED ${VERIFY_ARCHIVE}"
  exit 0
fi

# ── Backup mode ────────────────────────────────────────────────────────────
[ -n "${DATA_DIR}" ] || die "--data-dir is required"
[ -n "${OUT_DIR}" ] || die "--out is required"
[ -d "${DATA_DIR}" ] || die "no such data dir: ${DATA_DIR}"

DB="${DATA_DIR}/annex.db"
[ -f "${DB}" ] || die "no database at ${DB}"

mkdir -p "${OUT_DIR}"
stamp=$(date -u +%Y%m%d-%H%M%S)
stage=$(mktemp -d)
trap 'rm -rf "${stage}"' EXIT
mkdir -p "${stage}/annex-${stamp}"

# `.backup` rather than `cp`: under WAL a file copy can catch the database
# mid-transaction, and the -wal and -shm files it needs to be consistent are
# separate files that move independently. SQLite's own backup API takes a
# consistent snapshot of a LIVE database, which is the whole point — this runs
# against a running server.
info "snapshotting ${DB}"
sqlite3 "${DB}" ".backup '${stage}/annex-${stamp}/annex.db'"

check=$(sqlite3 "${stage}/annex-${stamp}/annex.db" 'PRAGMA integrity_check;' | head -1)
[ "${check}" = "ok" ] || { echo "[backup] snapshot failed integrity_check: ${check}" >&2; exit 2; }

if [ -f "${DATA_DIR}/signing.key" ]; then
  cp "${DATA_DIR}/signing.key" "${stage}/annex-${stamp}/signing.key"
  chmod 600 "${stage}/annex-${stamp}/signing.key"
else
  echo "[backup] WARNING no signing.key in ${DATA_DIR} — is this the right data dir?" >&2
fi

# Uploads are user data and are not in the database.
if [ -d "${DATA_DIR}/uploads" ]; then
  cp -r "${DATA_DIR}/uploads" "${stage}/annex-${stamp}/uploads"
fi

archive="${OUT_DIR}/annex-${stamp}.tar.gz"
tar -czf "${archive}" -C "${stage}" "annex-${stamp}"
chmod 600 "${archive}"   # it contains the signing key
info "wrote ${archive} ($(du -h "${archive}" | cut -f1))"

# Verify what was just written, not what was intended to be written.
bash "$0" --verify "${archive}" >/dev/null || { echo "[backup] the archive did not verify" >&2; exit 2; }
info "verified"

# ── Retention ──────────────────────────────────────────────────────────────
if [ "${KEEP}" -gt 0 ]; then
  count=$(find "${OUT_DIR}" -maxdepth 1 -name 'annex-*.tar.gz' | wc -l)
  if [ "${count}" -gt "${KEEP}" ]; then
    # Never prune below the keep count even if something else wrote files here,
    # and never prune the archive just written.
    find "${OUT_DIR}" -maxdepth 1 -name 'annex-*.tar.gz' | sort | head -n "$((count - KEEP))" |
      while IFS= read -r old; do
        [ "${old}" = "${archive}" ] && continue
        info "pruning ${old}"
        rm -f "${old}"
      done
  fi
fi

info "done"
