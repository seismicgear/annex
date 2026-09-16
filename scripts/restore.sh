#!/usr/bin/env bash
# restore.sh — restore an Annex deployment from a backup.sh archive.
#
# Stop the server first. This refuses to overwrite a live data directory,
# because a half-restored database under a running server is worse than either
# state on its own.
#
# Usage:
#   bash scripts/restore.sh --archive /backups/annex-20260914-120000.tar.gz --data-dir /app/data

set -euo pipefail

ARCHIVE=""
DATA_DIR=""
FORCE=0

die() { echo "[restore] ERROR $*" >&2; exit 1; }
info() { echo "[restore] $*"; }

while [ $# -gt 0 ]; do
  case "$1" in
    --archive)  ARCHIVE="${2:-}"; shift 2 ;;
    --data-dir) DATA_DIR="${2:-}"; shift 2 ;;
    --force)    FORCE=1; shift ;;
    -h|--help)  sed -n '2,12p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

[ -n "${ARCHIVE}" ] || die "--archive is required"
[ -n "${DATA_DIR}" ] || die "--data-dir is required"
[ -f "${ARCHIVE}" ] || die "no such archive: ${ARCHIVE}"

# Verify BEFORE touching anything. Discovering the archive is bad after moving
# the live data aside is the worst possible ordering.
info "verifying ${ARCHIVE}"
bash "$(dirname "$0")/backup.sh" --verify "${ARCHIVE}"

if [ -f "${DATA_DIR}/annex.db" ] && [ "${FORCE}" -ne 1 ]; then
  # A WAL file present usually means a server has the database open.
  if [ -f "${DATA_DIR}/annex.db-wal" ]; then
    die "${DATA_DIR}/annex.db-wal exists — stop the server first, or pass --force if you are sure"
  fi
  aside="${DATA_DIR}.pre-restore-$(date -u +%Y%m%d-%H%M%S)"
  info "moving the existing data dir aside to ${aside}"
  mv "${DATA_DIR}" "${aside}"
fi

mkdir -p "${DATA_DIR}"
tmp=$(mktemp -d)
trap 'rm -rf "${tmp}"' EXIT
tar -xzf "${ARCHIVE}" -C "${tmp}"
src=$(find "${tmp}" -maxdepth 1 -type d -name 'annex-*' | head -1)
[ -n "${src}" ] || die "archive has an unexpected layout"

cp "${src}/annex.db" "${DATA_DIR}/annex.db"
if [ -f "${src}/signing.key" ]; then
  cp "${src}/signing.key" "${DATA_DIR}/signing.key"
  chmod 600 "${DATA_DIR}/signing.key"
fi
[ -d "${src}/uploads" ] && cp -r "${src}/uploads" "${DATA_DIR}/uploads"

info "restored into ${DATA_DIR}. Start the server and check GET /readyz."
