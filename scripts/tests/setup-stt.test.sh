#!/usr/bin/env bash
# setup-stt.test.sh — the model installer must refuse a file it did not verify.
#
# The model is an input to a subprocess this server executes on call audio, so
# the interesting cases are all failure cases: a truncated download, a
# substituted file, a half-written file left by an interrupted run. A test that
# only proved the happy path would prove the least important thing.
#
# Driven against the real script with a stubbed `curl` on PATH, so no network
# and no 141 MiB download. The digests the script checks against are real.
#
# Run: bash scripts/tests/setup-stt.test.sh

set -uo pipefail

ROOT_DIR="$(cd "$(dirname "$0")/../.." && pwd)"
SCRIPT="${ROOT_DIR}/scripts/setup-stt.sh"

ST_OK=0
ST_BAD=0
st_ok()  { echo "[setup-stt-test] OK   $1"; ST_OK=$((ST_OK + 1)); }
st_bad() { echo "[setup-stt-test] FAIL $1" >&2; ST_BAD=$((ST_BAD + 1)); }

scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT

# A scratch project root: the script resolves `assets/models` from its own
# location, so it needs a copy of itself in a `scripts/` directory.
mkdir -p "$scratch/repo/scripts"
cp "$SCRIPT" "$scratch/repo/scripts/setup-stt.sh"
chmod +x "$scratch/repo/scripts/setup-stt.sh"

# The real pinned values for tiny.en, from the LFS pointer.
TINY_SHA="921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f"
TINY_BYTES=77704715

# ── A stubbed curl that writes whatever we tell it to ───────────────────────
mkdir -p "$scratch/bin"
make_curl() {
    # $1 = path to the payload this fake curl should deliver
    cat > "$scratch/bin/curl" <<EOF
#!/usr/bin/env bash
# Fake curl: ignore every flag, find -o, copy the payload there.
out=""
while [ \$# -gt 0 ]; do
  case "\$1" in
    -o) out="\$2"; shift 2 ;;
    *) shift ;;
  esac
done
[ -n "\$out" ] || exit 2
cp "$1" "\$out"
EOF
    chmod +x "$scratch/bin/curl"
}

run_script() {
    ( cd "$scratch/repo" && PATH="$scratch/bin:$PATH" bash scripts/setup-stt.sh "$@" 2>&1 )
}

# ── 1. An unknown model is refused, and says what it knows ──────────────────
out="$(run_script --model does-not-exist)"; rc=$?
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q "known models"; then
    st_ok "an unknown model name is refused and the known ones are listed"
else
    st_bad "an unknown model name should exit non-zero and list the known ones (rc=$rc): $out"
fi

# ── 2. A truncated download is refused on SIZE before the digest ────────────
#
# Checked separately from the digest because the messages differ and an
# operator debugging a flaky network needs "your download stopped early", not
# "this file may have been tampered with".
truncated="$scratch/truncated.bin"
head -c 1024 /dev/zero > "$truncated"
make_curl "$truncated"
out="$(run_script --model tiny.en)"; rc=$?
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -q "size mismatch"; then
    st_ok "a truncated download is refused, and named as a size mismatch"
else
    st_bad "a truncated download should fail on size (rc=$rc): $out"
fi

# ── 3. A right-sized WRONG file is refused on its digest ────────────────────
#
# The case a size check cannot catch, and the one that matters: a substituted
# model, or a corrupted one that happens to be the right length.
wrong="$scratch/wrong.bin"
head -c "$TINY_BYTES" /dev/zero > "$wrong"
make_curl "$wrong"
out="$(run_script --model tiny.en)"; rc=$?
if [ "$rc" -ne 0 ] && printf '%s' "$out" | grep -qi "DIGEST MISMATCH"; then
    st_ok "a right-sized file with the wrong contents is refused on its digest"
else
    st_bad "a wrong-content file should fail on its digest (rc=$rc): $(printf '%s' "$out" | tail -4)"
fi

# ── 4. A refused download leaves NOTHING behind ─────────────────────────────
#
# The `.part` name and the EXIT trap exist for this: a half-written file at the
# destination is indistinguishable from a complete one to the next run's
# `-f "$DEST"` check, and that run would report a digest mismatch that reads
# like tampering rather than an interrupted download.
if [ ! -e "$scratch/repo/assets/models/ggml-tiny.en.bin" ]; then
    st_ok "a refused download leaves no file at the destination"
else
    st_bad "a refused download left ggml-tiny.en.bin behind"
fi
if ls "$scratch/repo/assets/models/"*.part >/dev/null 2>&1; then
    st_bad "a refused download left a .part file behind"
else
    st_ok "a refused download leaves no .part file behind"
fi

# ── 5. --verify never downloads ─────────────────────────────────────────────
#
# An operator checking what is installed must not be given a model by the act
# of asking.
cat > "$scratch/bin/curl" <<'EOF'
#!/usr/bin/env bash
echo "curl was invoked during --verify" >&2
exit 99
EOF
chmod +x "$scratch/bin/curl"
out="$(run_script --model tiny.en --verify)"; rc=$?
if [ "$rc" -ne 0 ] && ! printf '%s' "$out" | grep -q "curl was invoked"; then
    st_ok "--verify reports the model is absent without downloading it"
else
    st_bad "--verify must not download (rc=$rc): $out"
fi

# ── 6. A correct file is accepted and installed ─────────────────────────────
#
# Last, so that every refusal above is known not to be the script simply
# failing at everything.
# Asserted with the REAL artifact rather than a fabricated one: the whole point
# of the script is that only the pinned bytes are accepted, so a fixture that
# satisfied it would have to BE the pinned bytes.
real="${ROOT_DIR}/assets/models/ggml-tiny.en.bin"
if [ -f "$real" ] && [ "$(sha256sum "$real" | cut -d' ' -f1)" = "$TINY_SHA" ]; then
    mkdir -p "$scratch/repo/assets/models"
    cp "$real" "$scratch/repo/assets/models/ggml-tiny.en.bin"
    out="$(run_script --model tiny.en --verify)"; rc=$?
    if [ "$rc" -eq 0 ] && printf '%s' "$out" | grep -q "ANNEX_STT_MODEL_PATH"; then
        st_ok "a correctly-pinned model verifies and the env line is printed"
    else
        st_bad "a correct model should verify (rc=$rc): $out"
    fi
else
    # Stated rather than skipped silently: this is the only assertion that the
    # script can succeed at all, and its absence is worth seeing.
    st_ok "no local ggml-tiny.en.bin — skipping the accept path (run scripts/setup-stt.sh --model tiny.en)"
fi

echo
echo "[setup-stt-test] ${ST_OK} passed, ${ST_BAD} failed"
[ "$ST_BAD" -eq 0 ]
