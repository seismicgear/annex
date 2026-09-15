#!/usr/bin/env bash
#
# Pins `scripts/baseline-drift.mjs` — the tool that answers "did this baseline
# actually change?" in pixels rather than in `git status` lines.
#
# The decoder is the part worth pinning. It implements PNG's five row filters by
# hand, and filters 3 (Average) and 4 (Paeth) are the ones an encoder picks for
# photographic rows — get either wrong and every pixel after the mistake is
# garbage, which does not look like a bug, it looks like a huge drift. So each
# filter gets a case with a KNOWN answer, built by encoding the same image twice
# and forcing a different filter byte each time.
set -uo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TOOL="${REPO_ROOT}/scripts/baseline-drift.mjs"

# Named so they cannot be absorbed by a sourced script's own counters — the
# `desktop-audit.sh` lesson in CLAUDE.md.
BD_PASS=0
BD_FAIL=0
bd_ok()  { BD_PASS=$((BD_PASS+1)); echo "[drift] OK   $1"; }
bd_bad() { BD_FAIL=$((BD_FAIL+1)); echo "[drift] FAIL $1"; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Minimal PNG encoder: RGBA, 8-bit, one filter type for the whole image.
mkpng() {
  node -e '
    const zlib = require("zlib"), fs = require("fs");
    const [out, w, h, filter, spec] = process.argv.slice(1);
    const W = +w, H = +h, F = +filter;
    // spec: "r,g,b,a" base colour, plus optional "@x,y=r,g,b,a" overrides.
    const parts = spec.split("@");
    const base = parts[0].split(",").map(Number);
    const px = Buffer.alloc(W * H * 4);
    for (let i = 0; i < W * H; i++) base.forEach((v, c) => { px[i * 4 + c] = v; });
    for (const o of parts.slice(1)) {
      const [pos, col] = o.split("=");
      const [x, y] = pos.split(",").map(Number);
      col.split(",").map(Number).forEach((v, c) => { px[(y * W + x) * 4 + c] = v; });
    }
    const stride = W * 4;
    const raw = Buffer.alloc(H * (stride + 1));
    for (let y = 0; y < H; y++) {
      raw[y * (stride + 1)] = F;
      for (let x = 0; x < stride; x++) {
        const cur = px[y * stride + x];
        const a = x >= 4 ? px[y * stride + x - 4] : 0;
        const b = y ? px[(y - 1) * stride + x] : 0;
        const c = y && x >= 4 ? px[(y - 1) * stride + x - 4] : 0;
        let v;
        if (F === 0) v = cur;
        else if (F === 1) v = cur - a;
        else if (F === 2) v = cur - b;
        else if (F === 3) v = cur - ((a + b) >> 1);
        else {
          const p = a + b - c, pa = Math.abs(p - a), pb = Math.abs(p - b), pc = Math.abs(p - c);
          v = cur - (pa <= pb && pa <= pc ? a : pb <= pc ? b : c);
        }
        raw[y * (stride + 1) + 1 + x] = v & 0xff;
      }
    }
    const chunk = (type, data) => {
      const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
      const body = Buffer.concat([Buffer.from(type, "ascii"), data]);
      const crcTable = [];
      for (let n = 0; n < 256; n++) { let c = n; for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1; crcTable[n] = c >>> 0; }
      let crc = 0xffffffff;
      for (const byte of body) crc = crcTable[(crc ^ byte) & 0xff] ^ (crc >>> 8);
      const crcBuf = Buffer.alloc(4); crcBuf.writeUInt32BE((crc ^ 0xffffffff) >>> 0);
      return Buffer.concat([len, body, crcBuf]);
    };
    const ihdr = Buffer.alloc(13);
    ihdr.writeUInt32BE(W, 0); ihdr.writeUInt32BE(H, 4);
    ihdr[8] = 8; ihdr[9] = 6; ihdr[10] = 0; ihdr[11] = 0; ihdr[12] = 0;
    fs.writeFileSync(out, Buffer.concat([
      Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
      chunk("IHDR", ihdr), chunk("IDAT", zlib.deflateSync(raw)), chunk("IEND", Buffer.alloc(0)),
    ]));
  ' "$@"
}

ratio_of() { node -e 'const r=JSON.parse(require("fs").readFileSync(0,"utf8"));process.stdout.write(String(r.differing ?? "size"))'; }

# ── identical images, every filter type ─────────────────────────────────────
# The decoder has to reconstruct the same pixels no matter which filter the
# encoder chose; a filter implemented wrongly shows up here as a huge count.
#
# The Paeth neighbourhood at (11,11) is deliberate. Its predictor distances are
# pa=|b-c|=20, pb=|a-c|=40, pc=|a+b-2c|=20 — so pa TIES pc, and the spec's
# `pa <= pb && pa <= pc` picks `a` while the common mis-write `pa < pb && pa <
# pc` falls through and picks `c`, 40 levels away. Without a tie in the fixture
# both spellings decode identically and this case asserts nothing: the first
# version of this test passed against a decoder with exactly that mistake
# injected. A tie needs a = c - 2(b - c), which is where 160/220/200 comes from.
PAETH_TIE="@10,10=200,200,200,255@11,10=220,220,220,255@10,11=160,160,160,255@11,11=60,60,60,255"
for f in 0 1 2 3 4; do
  mkpng "$WORK/a$f.png" 40 30 "$f" "10,20,30,255@5,5=200,10,10,255@30,25=0,255,0,255${PAETH_TIE}"
  mkpng "$WORK/b$f.png" 40 30 0   "10,20,30,255@5,5=200,10,10,255@30,25=0,255,0,255${PAETH_TIE}"
  d="$(node "$TOOL" --json "$WORK/a$f.png" "$WORK/b$f.png" | ratio_of)"
  if [ "$d" = "0" ]; then
    bd_ok "filter $f decodes to the same pixels as filter 0"
  else
    bd_bad "filter $f decoded differently from filter 0 ($d differing pixels)"
  fi
done

# ── a known number of differing pixels ──────────────────────────────────────
mkpng "$WORK/base.png" 40 30 4 "10,20,30,255"
mkpng "$WORK/two.png"  40 30 4 "10,20,30,255@1,1=255,255,255,255@2,2=255,255,255,255"
d="$(node "$TOOL" --json "$WORK/base.png" "$WORK/two.png" | ratio_of)"
[ "$d" = "2" ] && bd_ok "two changed pixels are counted as two" \
                || bd_bad "expected 2 differing pixels, got $d"

# ── the channel threshold is a tolerance, not equality ──────────────────────
# A +10 shift is anti-aliasing noise and must not count; the default is 20.
mkpng "$WORK/near.png" 40 30 4 "20,30,40,255"
d="$(node "$TOOL" --json "$WORK/base.png" "$WORK/near.png" | ratio_of)"
[ "$d" = "0" ] && bd_ok "a +10 per-channel shift is within tolerance" \
                || bd_bad "a +10 shift counted $d pixels — the threshold is not applied"
d="$(BASELINE_DRIFT_THRESHOLD=5 node "$TOOL" --json "$WORK/base.png" "$WORK/near.png" | ratio_of)"
[ "$d" = "1200" ] && bd_ok "lowering the threshold counts the same shift" \
                   || bd_bad "at threshold 5 expected all 1200 pixels, got $d"

# ── a resized capture is reported as resized, never as a ratio ──────────────
mkpng "$WORK/tall.png" 40 31 4 "10,20,30,255"
out="$(node "$TOOL" --json "$WORK/base.png" "$WORK/tall.png")"
if node -e 'const r=JSON.parse(process.argv[1]); process.exit(r.sizeChanged && r.from==="40x30" && r.to==="40x31" ? 0 : 1)' "$out"; then
  bd_ok "a changed clip is reported as a size change, not a drift ratio"
else
  bd_bad "size change not reported: $out"
fi

# ── exit status carries the verdict ─────────────────────────────────────────
node "$TOOL" "$WORK/base.png" "$WORK/base.png" >/dev/null 2>&1
[ $? -eq 0 ] && bd_ok "identical images exit 0" || bd_bad "identical images did not exit 0"
node "$TOOL" "$WORK/base.png" "$WORK/tall.png" >/dev/null 2>&1
[ $? -eq 1 ] && bd_ok "a size change exits 1" || bd_bad "a size change did not exit 1"

echo
echo "[drift] ${BD_PASS} passed, ${BD_FAIL} failed"
[ "$BD_FAIL" -eq 0 ]
