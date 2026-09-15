#!/usr/bin/env bash
# roadmap-consistency.test.sh — ROADMAP.md must agree with itself.
#
# ROADMAP.md calls itself "the single source of truth for what's built" and
# carries the status of each phase in TWO places: the `Current State` block
# near the top, and a `**Status**:` line in each `## Phase N` section.
#
# They disagreed in five places. On 2026-06-19 a "reality check" downgraded
# Phases 1, 3, 6, 7 and 9 to PARTIAL in the block, with the specific gaps
# noted — and every one of those phase sections still said `COMPLETE`, with
# Phases 6 and 9 additionally carrying a full set of ticked completion
# criteria. A reader who opened the document at the phase they cared about got
# the opposite answer from a reader who read the summary.
#
# It drifted because nothing checked it. That is the same reason the
# IMMEDIATE-transaction rule was re-broken three times after being written
# down, and it got the same treatment: a check instead of a paragraph.
#
# Run: bash scripts/tests/roadmap-consistency.test.sh

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
ROADMAP="${ROOT_DIR}/ROADMAP.md"

# Deliberately not PASS/FAIL: `desktop-audit.sh` keeps counters by those names
# and a test that sourced it had its tally silently absorbed. Nothing is
# sourced here, but the convention is cheap and the bug was not.
RC_OK=0
RC_BAD=0

note_ok() { echo "[roadmap] OK   $1"; RC_OK=$((RC_OK + 1)); }
note_bad() { echo "[roadmap] FAIL $1" >&2; RC_BAD=$((RC_BAD + 1)); }

if [ ! -f "${ROADMAP}" ]; then
  echo "[roadmap] FAIL ROADMAP.md not found at ${ROADMAP}" >&2
  exit 1
fi

# ── The Current State block ────────────────────────────────────────────────
#
# Lines look like:  `Phase 7: Voice Infrastructure .......... PARTIAL  (…)`
# The status is the first all-caps word after the dot leader.
declare -A BLOCK
while IFS= read -r line; do
  phase=$(printf '%s' "${line}" | sed -n 's/^Phase \([0-9]\{1,2\}\):.*/\1/p')
  [ -n "${phase}" ] || continue
  status=$(printf '%s' "${line}" | sed -n 's/.*\.\.\.* *\([A-Z]\{4,\}\).*/\1/p')
  [ -n "${status}" ] || continue
  BLOCK["${phase}"]="${status}"
done < <(sed -n '/^## Current State/,/^## Code Standards/p' "${ROADMAP}")

if [ "${#BLOCK[@]}" -eq 0 ]; then
  note_bad "could not parse any phase status out of the Current State block — has its format changed?"
else
  note_ok "parsed ${#BLOCK[@]} phase statuses from the Current State block"
fi

# ── Each phase section's own Status line ───────────────────────────────────
declare -A SECTION
current=""
while IFS= read -r line; do
  heading=$(printf '%s' "${line}" | sed -n 's/^## Phase \([0-9]\{1,2\}\):.*/\1/p')
  if [ -n "${heading}" ]; then
    current="${heading}"
    continue
  fi
  case "${line}" in
    '**Status**:'*)
      [ -n "${current}" ] || continue
      # `**Status**: `PARTIAL` — reason.`  →  PARTIAL
      status=$(printf '%s' "${line}" | sed -n 's/^\*\*Status\*\*: *`\([A-Z]\{4,\}\)`.*/\1/p')
      [ -n "${status}" ] || status="UNPARSEABLE"
      SECTION["${current}"]="${status}"
      current=""
      ;;
  esac
done < "${ROADMAP}"

# ── Compare ────────────────────────────────────────────────────────────────
for phase in $(printf '%s\n' "${!BLOCK[@]}" | sort -n); do
  want="${BLOCK[$phase]}"
  got="${SECTION[$phase]:-MISSING}"
  if [ "${want}" = "${got}" ]; then
    note_ok "Phase ${phase}: both say ${want}"
  else
    note_bad "Phase ${phase}: the Current State block says ${want}, the phase section says ${got}"
  fi
done

# A phase section with no counterpart in the block is just as much a drift.
for phase in $(printf '%s\n' "${!SECTION[@]}" | sort -n); do
  if [ -z "${BLOCK[$phase]:-}" ]; then
    note_bad "Phase ${phase} has a section but no line in the Current State block"
  fi
done

# ── A PARTIAL phase has to say what is missing ─────────────────────────────
#
# Without this the cheapest way to make the check above pass is to write
# PARTIAL in both places and explain nothing, which loses the information the
# 2026-06-19 reality check was written to preserve.
while IFS= read -r line; do
  case "${line}" in
    '**Status**: `PARTIAL`'*)
      reason=$(printf '%s' "${line}" | sed -n 's/^\*\*Status\*\*: *`PARTIAL` *—* *//p')
      if [ ${#reason} -lt 20 ]; then
        note_bad "a PARTIAL phase does not name its gap: ${line}"
      fi
      ;;
  esac
done < "${ROADMAP}"
note_ok "every PARTIAL phase names the gap that keeps it partial"

# ── A phase's criteria have to agree with its own status ───────────────────
#
# Phase 1 said `PARTIAL` with a named gap (the ceremony type) while every one
# of its ten completion criteria was unchecked, and the changelog recorded
# "Phase 1 COMPLETE" three months earlier. Three statements in one document,
# no two of which could both be true, and the check above was satisfied by all
# of it because it only compared the two Status lines to each other.
#
# The rule: a phase whose Status is PARTIAL or COMPLETE must have at least one
# ticked criterion. A wholly-unticked criteria list means either the work is
# not done (so the Status is wrong) or the boxes were never maintained (so the
# list is decoration).
awk '
  /^## Phase [0-9]+/            { phase = $3; sub(/:$/, "", phase); incrit = "" }
  /^\*\*Status\*\*: `(PARTIAL|COMPLETE)`/ && phase != "" { status[phase] = 1 }
  /^### Completion Criteria/    { incrit = phase }
  /^## / && !/^## Phase/        { incrit = "" }
  incrit != "" && /^- \[x\]/    { ticked[incrit]++ }
  incrit != "" && /^- \[ \]/    { unticked[incrit]++ }
  END {
    for (p in status) {
      if ((ticked[p] + unticked[p]) > 0 && ticked[p] == 0) {
        printf "UNTICKED %s %d\n", p, unticked[p]
      }
    }
  }
' "${ROADMAP}" > "${TMPDIR:-/tmp}/roadmap-criteria.$$" || true

if [ -s "${TMPDIR:-/tmp}/roadmap-criteria.$$" ]; then
  while read -r _ phase count; do
    note_bad "Phase ${phase} claims a status but all ${count} of its completion criteria are unchecked"
  done < "${TMPDIR:-/tmp}/roadmap-criteria.$$"
else
  note_ok "no phase claims a status with a wholly-unchecked criteria list"
fi
rm -f "${TMPDIR:-/tmp}/roadmap-criteria.$$"

echo "[roadmap] ${RC_OK} passed, ${RC_BAD} failed"
[ "${RC_BAD}" -eq 0 ] || exit 1
