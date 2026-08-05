#!/usr/bin/env bash
#
# desktop-audit.sh — Journey stage 01: app install.
#
# The web audit (`scripts/ui-audit.sh`) drives the SPA served over HTTP, which
# can reach everything EXCEPT the parts that only exist in the desktop shell:
# the packaged installer, the `annex://` protocol handler the OS registers at
# install time, the first-run data wipe, the embedded server, and the reset
# path. Those are the first things a real user touches, so they get their own
# lane.
#
# Three layers, cheapest first, because each one catches a different class of
# break and the expensive ones are worth skipping when a cheap one already
# failed:
#
#   1. compile   — `cargo check` + `clippy` on the Tauri crate.
#   2. logic     — the crate's own unit tests (deep-link parsing, startup
#                  prefs, config, media detection). CI skips these because
#                  linking every Tauri dep twice exhausts a standard runner's
#                  disk; this script checks for headroom and says so.
#   3. package   — build the .deb, install it, confirm the binary and the
#                  `annex://` handler land on the system, launch it headless
#                  under Xvfb, then uninstall and confirm removal.
#
# Usage:
#   bash scripts/desktop-audit.sh              # all three layers
#   bash scripts/desktop-audit.sh --no-package # skip the bundle/install cycle
#
# Requires the GTK/WebKit/soup/pipewire dev packages (see
# scripts/claude-setup.sh) and, for layer 3, root for dpkg.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"

PACKAGE=1
[ "${1:-}" = "--no-package" ] && PACKAGE=0

PASS=0
FAIL=0
step() {
    local name="$1"; shift
    echo ""
    echo "── $name ──"
    if "$@"; then
        echo "   ok"
        PASS=$((PASS + 1))
    else
        echo "   FAILED"
        FAIL=$((FAIL + 1))
    fi
}

# ── Preconditions ────────────────────────────────────────────────────────
if ! pkg-config --exists webkit2gtk-4.1; then
    echo "[desktop-audit] missing WebKitGTK dev packages — run scripts/claude-setup.sh" >&2
    exit 1
fi

# The Tauri bundler validates `bundle.resources` at build time, and
# assets/piper + assets/voices are gitignored (Piper is fetched at deploy
# time). Their .gitkeep stubs are tracked, but a stale checkout may lack them.
mkdir -p assets/piper assets/voices

echo "[desktop-audit] webkit2gtk $(pkg-config --modversion webkit2gtk-4.1)"
echo "[desktop-audit] free disk: $(df -h --output=avail / | tail -1 | tr -d ' ')"

# ── 1. Compile ───────────────────────────────────────────────────────────
step "cargo check -p annex-desktop" cargo check -p annex-desktop
step "cargo clippy -p annex-desktop" \
    cargo clippy -p annex-desktop --all-targets -- -D warnings

# ── 2. Logic ─────────────────────────────────────────────────────────────
# The test binary links gtk/wry/webkit2gtk a second time on top of the lib,
# which is what exhausts CI runners. Check for room rather than discovering it
# halfway through a link.
avail_kb=$(df --output=avail / | tail -1 | tr -d ' ')
if [ "$avail_kb" -lt 8000000 ]; then
    echo ""
    echo "── cargo test -p annex-desktop ──"
    echo "   SKIPPED: needs ~8 GB free for the second link, have $((avail_kb / 1024)) MB"
else
    step "cargo test -p annex-desktop" cargo test -p annex-desktop
fi

# ── 3. Package: build → install → launch → uninstall ─────────────────────
if [ "$PACKAGE" -eq 0 ]; then
    echo ""
    echo "[desktop-audit] skipping the package cycle (--no-package)"
elif [ "$(id -u)" -ne 0 ]; then
    echo ""
    echo "[desktop-audit] skipping the package cycle (dpkg needs root)"
else
    if ! command -v cargo-tauri >/dev/null 2>&1; then
        echo ""
        echo "[desktop-audit] installing tauri-cli (one-off, several minutes)"
        cargo install tauri-cli@2.11.2 --locked || true
    fi

    # SKIP_PIPER keeps the build from reaching out to GitHub for the TTS
    # binary; packaging correctness does not depend on it being present.
    step "cargo tauri build --debug --bundles deb" \
        env SKIP_PIPER=1 ANNEX_BUILD_PROFILE=dev \
        cargo tauri build --debug --bundles deb --manifest-path crates/annex-desktop/Cargo.toml

    deb=$(find target -name 'Annex_*.deb' -o -name 'annex_*.deb' 2>/dev/null | head -1)
    if [ -z "$deb" ]; then
        echo "   FAILED: no .deb produced"
        FAIL=$((FAIL + 1))
    else
        echo "[desktop-audit] built $deb"
        step "dpkg -i (install)" dpkg -i "$deb"

        step "installed binary is on PATH" bash -c 'command -v annex-desktop >/dev/null'

        # This is the whole point of the install step: the OS has to learn the
        # annex:// scheme, or every invite link a user clicks goes nowhere.
        step "annex:// scheme handler registered" bash -c \
            'grep -q "x-scheme-handler/annex" /usr/share/applications/*.desktop'

        # Launch headless. The app is expected to still be running after a few
        # seconds — an immediate exit means it crashed on startup.
        echo ""
        echo "── launch under Xvfb ──"
        xvfb-run -a annex-desktop >/tmp/annex-desktop-launch.log 2>&1 &
        launch_pid=$!
        sleep 12
        if kill -0 "$launch_pid" 2>/dev/null; then
            echo "   ok — still running after 12s"
            PASS=$((PASS + 1))
            kill "$launch_pid" 2>/dev/null || true
            wait "$launch_pid" 2>/dev/null || true
        else
            echo "   FAILED — exited early; log:"
            tail -20 /tmp/annex-desktop-launch.log | sed 's/^/     /'
            FAIL=$((FAIL + 1))
        fi

        step "dpkg -r (uninstall)" dpkg -r annex
        step "binary removed" bash -c '! command -v annex-desktop >/dev/null'
    fi
fi

echo ""
echo "═══════════════════════════════════════"
echo "  desktop audit: $PASS passed, $FAIL failed"
echo "═══════════════════════════════════════"
[ "$FAIL" -eq 0 ]
