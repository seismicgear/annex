#!/usr/bin/env bash
# test-all.sh — Unified test runner for Annex.
# Runs formatting, linting, Rust tests, and frontend tests.
#
# Usage:
#   bash scripts/test-all.sh            # full suite
#   bash scripts/test-all.sh --quick    # skip fmt/clippy
#   bash scripts/test-all.sh --verbose  # cargo test with --nocapture
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
RED='\033[0;31m'
BOLD='\033[1m'
NC='\033[0m'

QUICK=false
VERBOSE=false
CARGO_TEST_EXTRA=""

for arg in "$@"; do
    case "$arg" in
        --quick)   QUICK=true ;;
        --verbose) VERBOSE=true; CARGO_TEST_EXTRA="-- --nocapture" ;;
    esac
done

FAILURES=()
PASS_COUNT=0
FAIL_COUNT=0

run_step() {
    local label="$1"
    shift
    echo ""
    echo -e "${BOLD}>>> ${label}${NC}"
    if "$@"; then
        echo -e "${GREEN}  PASSED${NC}: ${label}"
        PASS_COUNT=$((PASS_COUNT + 1))
    else
        echo -e "${RED}  FAILED${NC}: ${label}"
        FAILURES+=("$label")
        FAIL_COUNT=$((FAIL_COUNT + 1))
    fi
}

# ---------- Formatting ----------
if [ "$QUICK" = false ]; then
    run_step "cargo fmt --check" cargo fmt --all --check
fi

# ---------- Clippy ----------
if [ "$QUICK" = false ]; then
    run_step "cargo clippy" cargo clippy --workspace --exclude annex-desktop --all-targets -- -D warnings
fi

# ---------- Frontend lint ----------
# With clippy, not with the tests: it is the frontend's linter and `--quick`
# is documented as skipping linting.
if [ "$QUICK" = false ]; then
    run_step "eslint (Frontend)" bash -c "cd client && npm run lint"
fi

# ---------- Harness scripts ----------
# The scripts everything else is run through. Their failures were the silent
# kind: e2e-server.sh reported "Killing stray process" having killed nothing
# and then "Server ready" against the survivor; claude-setup.sh ended on a
# failed apt-get with no statement of what failed, because `set -e` with
# `pipefail` aborts at the pipeline and the check sat after it. Seconds each,
# and nothing else covers a shell script. Globbed, so a new one is picked up.
for _t in scripts/tests/*.test.sh; do
    [ -e "$_t" ] || continue
    run_step "$(basename "$_t" .test.sh) (harness)" bash "$_t"
done

# ---------- Rust tests ----------
if [ -n "$CARGO_TEST_EXTRA" ]; then
    run_step "cargo test (Rust)" cargo test --workspace --exclude annex-desktop $CARGO_TEST_EXTRA
else
    run_step "cargo test (Rust)" cargo test --workspace --exclude annex-desktop
fi

# ---------- Frontend typecheck ----------
# Always, including under --quick, because this is the frontend's COMPILE
# step and `--quick` skips linting, not compiling.
#
# Vitest transpiles with esbuild, which strips types without checking them, so
# `npm test` passes on a tree that does not compile — verified by adding
# `const x: number = "s"` to a source file: `tsc -b` failed and all 469 tests
# passed. This script is what CLAUDE.md calls the recommended way to test, and
# it was reporting "All steps passed" on code that could not be built.
run_step "tsc (Frontend types)" bash -c "cd client && npx tsc -b"

# ---------- Frontend tests ----------
run_step "npm test (Frontend)" bash -c "cd client && npm test"

# ---------- Summary ----------
echo ""
echo -e "${BOLD}=== Test Summary ===${NC}"
echo -e "  Steps passed: ${GREEN}${PASS_COUNT}${NC}"
echo -e "  Steps failed: ${RED}${FAIL_COUNT}${NC}"

if [ ${#FAILURES[@]} -gt 0 ]; then
    echo -e "\n${RED}Failed steps:${NC}"
    for f in "${FAILURES[@]}"; do
        echo -e "  - ${f}"
    done
    exit 1
else
    echo -e "\n${GREEN}All steps passed.${NC}"
    exit 0
fi
