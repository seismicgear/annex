#!/usr/bin/env bash
# setup-embedding-model.sh — fetch and verify the VRP semantic embedding model.
#
# Usage:
#   ./scripts/setup-embedding-model.sh            # fetch if missing, verify always
#   ./scripts/setup-embedding-model.sh --verify   # verify only, never download
#
# Creates:
#   assets/embedding/model.safetensors   — 29528 x 64 f32 static embedding table
#   assets/embedding/tokenizer.json      — the WordPiece tokenizer it was built with
#
# ── Why a static embedding table and not a transformer ──────────────────────
#
# The model is `minishlab/potion-base-2M` (MIT). It is a *static* embedding —
# a token→vector lookup distilled from BGE-base-en-v1.5 — so embedding a
# sentence is a table lookup and a mean, with no inference graph, no runtime,
# and no matrix multiply. That matters three times over here:
#
#   * **Size.** 7.5 MB, against ~90 MB for all-MiniLM-L6-v2 in ONNX. This ships
#     inside every desktop installer and every container image.
#   * **Determinism.** The output of this comparison decides Aligned / Partial /
#     Conflict for an agent or a federation peer — a trust decision, not a
#     search ranking. A mean of f32 rows summed in a fixed token order is
#     reproducible; a transformer's GEMM kernels are not reproducible across
#     BLAS backends and CPU feature sets, and two peers scoring the same pair
#     differently is a disagreement about who is trustworthy.
#   * **Dependencies.** No onnxruntime, no C++ toolchain, no platform-specific
#     shared object in the bundle.
#
# ── Why the hashes are pinned ───────────────────────────────────────────────
#
# Two servers must reach the same verdict about the same pair of principle
# sets. A peer running a different revision of the model would score
# differently and neither side would know — the handshake would simply produce
# an alignment nobody could reproduce. The digests below are the contract; the
# model id and digest also travel in the VRP handshake so a mismatch is
# *detected* rather than silently tolerated.
#
# Pinned from revision 389b9f64be5aa4ae7a6bc6fe95ef20ce485ae5da.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

MODEL_ID="minishlab/potion-base-2M"
MODEL_REVISION="389b9f64be5aa4ae7a6bc6fe95ef20ce485ae5da"
BASE_URL="https://huggingface.co/${MODEL_ID}/resolve/${MODEL_REVISION}"
DEST_DIR="$PROJECT_ROOT/assets/embedding"

# file:sha256
FILES=(
  "model.safetensors:f95ffde02ad06f63ae38eb9d400038cd5ccaf8411ec3cb650c6025113f96cbb8"
  "tokenizer.json:e67e803f624fb4d67dea1c730d06e1067e1b14d830e2c2202569e3ef0f70bb50"
)

VERIFY_ONLY=0
for arg in "$@"; do
  case "$arg" in
    --verify) VERIFY_ONLY=1 ;;
    -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

info() { echo ":: $1"; }
ok()   { echo "   OK: $1"; }
fail() { echo "   FAIL: $1" >&2; exit 1; }

command -v curl >/dev/null 2>&1 || fail "curl is required"
command -v sha256sum >/dev/null 2>&1 || command -v shasum >/dev/null 2>&1 \
  || fail "sha256sum or shasum is required"

digest_of() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | cut -d' ' -f1
  else
    shasum -a 256 "$1" | cut -d' ' -f1
  fi
}

mkdir -p "$DEST_DIR"

info "VRP embedding model: ${MODEL_ID} @ ${MODEL_REVISION:0:12}"

for entry in "${FILES[@]}"; do
  name="${entry%%:*}"
  want="${entry##*:}"
  dest="$DEST_DIR/$name"

  if [ -f "$dest" ]; then
    got="$(digest_of "$dest")"
    if [ "$got" = "$want" ]; then
      ok "$name (verified)"
      continue
    fi
    # Not "already present, moving on". A file whose digest does not match is
    # a different model, and silently keeping it is how two deployments end up
    # scoring the same principles differently.
    if [ "$VERIFY_ONLY" -eq 1 ]; then
      fail "$name digest mismatch
     expected $want
     got      $got
   The model on disk is not the pinned revision. Re-run without --verify to refetch."
    fi
    info "$name digest mismatch — refetching"
    rm -f "$dest"
  elif [ "$VERIFY_ONLY" -eq 1 ]; then
    fail "$name is missing at $dest (run scripts/setup-embedding-model.sh)"
  fi

  info "downloading $name"
  # Download to a temporary path and only move it into place once verified, so
  # an interrupted transfer cannot leave a truncated file that later looks
  # present.
  tmp="$(mktemp "${dest}.XXXXXX")"
  if ! curl -fsSL --retry 3 --retry-delay 2 -o "$tmp" "${BASE_URL}/${name}"; then
    rm -f "$tmp"
    fail "could not download ${BASE_URL}/${name}"
  fi
  got="$(digest_of "$tmp")"
  if [ "$got" != "$want" ]; then
    rm -f "$tmp"
    fail "$name digest mismatch after download
     expected $want
     got      $got
   Either the pin is stale or the download was tampered with. Not installing it."
  fi
  mv "$tmp" "$dest"
  chmod 0644 "$dest"
  ok "$name (downloaded and verified)"
done

info "embedding model ready at assets/embedding"
