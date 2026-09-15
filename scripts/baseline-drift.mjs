#!/usr/bin/env node
//
// What a baseline change actually is, in pixels.
//
// CLAUDE.md: "`git status` is not a measure of visual change — count pixels."
// PNG bytes are not reproducible run to run, so re-recording a baseline that is
// visually identical still rewrites it: font rasterisation moves a handful of
// anti-aliased pixels and the file hash changes. Measured on this repo, an
// unchanged `.chat-area` capture comes back with 8-16 differing pixels out of
// 711,760 — 0.00002 against the 0.005 tolerance, 250x below it. So a `git
// status` listing 54 modified baselines can mean nothing moved, and one such
// claim in this repo's history was about 2x overstated because it counted files
// instead of pixels.
//
// That rule has been in CLAUDE.md for several sessions with nothing implementing
// it, which is the same shape as every other rule here that got re-broken: the
// only thing enforcing it was whether the next person had read the paragraph.
//
//   node scripts/baseline-drift.mjs                  # every changed baseline vs HEAD
//   node scripts/baseline-drift.mjs a.png b.png      # two files
//   node scripts/baseline-drift.mjs --json           # one JSON object per line
//
// Reports, per file: dimensions, differing pixels, the ratio, and whether the
// ratio clears `maxDiffPixelRatio` (0.005). A SIZE CHANGE is called out
// separately and never treated as a ratio — a capture whose clip moved is a
// different picture, not a drifted one, and averaging it into a ratio hides it.
//
// No dependencies on purpose. zlib inflate plus the five PNG filter types is
// the whole of what a non-interlaced 8-bit image needs, and Playwright's
// baselines are exactly that, so this runs anywhere node does — including a
// bisect where `client/node_modules` is from the wrong commit.
import { execFileSync } from "node:child_process";
import { readFileSync, existsSync } from "node:fs";
import { inflateSync } from "node:zlib";
import path from "node:path";

// Playwright's default per-channel tolerance for "this pixel is the same
// colour", and the ratio of differing pixels it allows over the whole image.
// Both mirror `client/playwright.config.ts`; if that file changes, change these
// and say so in the same commit.
const CHANNEL_THRESHOLD = Number(process.env.BASELINE_DRIFT_THRESHOLD ?? 20);
const MAX_DIFF_PIXEL_RATIO = Number(process.env.BASELINE_DRIFT_MAX_RATIO ?? 0.005);

const BASELINES = "client/e2e/audit/baselines";

export function decodePng(buf, label = "<buffer>") {
  if (buf.length < 8 || buf.readUInt32BE(0) !== 0x89504e47) {
    throw new Error(`${label}: not a PNG`);
  }
  let off = 8;
  let width = 0, height = 0, depth = 0, colour = 0, interlace = 0;
  const idat = [];
  while (off + 8 <= buf.length) {
    const len = buf.readUInt32BE(off);
    const type = buf.toString("ascii", off + 4, off + 8);
    const data = buf.subarray(off + 8, off + 8 + len);
    if (type === "IHDR") {
      width = data.readUInt32BE(0);
      height = data.readUInt32BE(4);
      depth = data[8];
      colour = data[9];
      interlace = data[12];
    } else if (type === "IDAT") {
      idat.push(data);
    } else if (type === "IEND") {
      break;
    }
    off += 12 + len;
  }
  if (depth !== 8) throw new Error(`${label}: bit depth ${depth} is not supported`);
  if (interlace !== 0) throw new Error(`${label}: interlaced PNGs are not supported`);
  const channels = { 0: 1, 2: 3, 3: 1, 4: 2, 6: 4 }[colour];
  if (!channels) throw new Error(`${label}: colour type ${colour} is not supported`);

  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * channels;
  const out = Buffer.alloc(height * stride);
  let p = 0;
  for (let y = 0; y < height; y++) {
    const filter = raw[p++];
    const line = raw.subarray(p, p + stride);
    p += stride;
    const cur = out.subarray(y * stride, (y + 1) * stride);
    const prev = y ? out.subarray((y - 1) * stride, y * stride) : null;
    for (let x = 0; x < stride; x++) {
      const a = x >= channels ? cur[x - channels] : 0;
      const b = prev ? prev[x] : 0;
      const c = prev && x >= channels ? prev[x - channels] : 0;
      let v = line[x];
      if (filter === 1) v += a;
      else if (filter === 2) v += b;
      else if (filter === 3) v += (a + b) >> 1;
      else if (filter === 4) {
        const pp = a + b - c;
        const pa = Math.abs(pp - a), pb = Math.abs(pp - b), pc = Math.abs(pp - c);
        v += pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
      } else if (filter !== 0) {
        throw new Error(`${label}: unknown filter ${filter} on row ${y}`);
      }
      cur[x] = v & 0xff;
    }
  }
  return { width, height, channels, data: out };
}

export function comparePng(bufA, bufB, label = "") {
  const A = decodePng(bufA, `${label} (a)`);
  const B = decodePng(bufB, `${label} (b)`);
  if (A.width !== B.width || A.height !== B.height) {
    return {
      label, sizeChanged: true,
      from: `${A.width}x${A.height}`, to: `${B.width}x${B.height}`,
    };
  }
  const pixels = A.width * A.height;
  const ch = Math.min(A.channels, B.channels);
  let differing = 0;
  for (let i = 0; i < pixels; i++) {
    const ia = i * A.channels, ib = i * B.channels;
    for (let c = 0; c < ch; c++) {
      if (Math.abs(A.data[ia + c] - B.data[ib + c]) > CHANNEL_THRESHOLD) {
        differing++;
        break;
      }
    }
  }
  return {
    label, sizeChanged: false,
    size: `${A.width}x${A.height}`, pixels, differing,
    ratio: differing / pixels,
    overTolerance: differing / pixels > MAX_DIFF_PIXEL_RATIO,
  };
}

function changedBaselines() {
  // `-z` because a path with a space would otherwise be split; the audit has
  // none today and that is not a reason to write it so it breaks when one does.
  const out = execFileSync("git", ["status", "--porcelain", "-z", "--", BASELINES], {
    encoding: "utf8", maxBuffer: 64 * 1024 * 1024,
  });
  const rows = [];
  for (const entry of out.split("\0")) {
    if (!entry) continue;
    const status = entry.slice(0, 2);
    const file = entry.slice(3);
    if (!file.endsWith(".png")) continue;
    rows.push({ status: status.trim(), file });
  }
  return rows;
}

function head(file) {
  try {
    // stderr silenced: an untracked baseline makes `git show` print a fatal,
    // and a new surface's first recording is an ordinary thing, not an error.
    return execFileSync("git", ["show", `HEAD:${file}`], {
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["ignore", "pipe", "ignore"],
    });
  } catch {
    return null;
  }
}

function main() {
  const argv = process.argv.slice(2);
  const json = argv.includes("--json");
  const files = argv.filter((a) => !a.startsWith("--"));

  if (files.length === 2) {
    const r = comparePng(readFileSync(files[0]), readFileSync(files[1]), files[1]);
    process.stdout.write(json ? JSON.stringify(r) + "\n" : render([r]));
    process.exit(r.sizeChanged || r.overTolerance ? 1 : 0);
  }
  if (files.length) {
    process.stderr.write("usage: baseline-drift.mjs [--json] [<a.png> <b.png>]\n");
    process.exit(2);
  }

  const results = [];
  for (const { status, file } of changedBaselines()) {
    if (status === "D") { results.push({ label: file, deleted: true }); continue; }
    const before = head(file);
    if (!before) { results.push({ label: file, added: true }); continue; }
    if (!existsSync(file)) { results.push({ label: file, deleted: true }); continue; }
    results.push(comparePng(before, readFileSync(file), file));
  }
  if (json) {
    for (const r of results) process.stdout.write(JSON.stringify(r) + "\n");
  } else {
    process.stdout.write(render(results));
  }
}

function render(results) {
  if (!results.length) return "no changed baselines\n";
  const lines = [];
  const moved = results.filter((r) => r.overTolerance);
  const resized = results.filter((r) => r.sizeChanged);
  const noise = results.filter((r) => !r.sizeChanged && !r.overTolerance && !r.added && !r.deleted);
  const added = results.filter((r) => r.added);
  const deleted = results.filter((r) => r.deleted);

  const name = (r) => path.relative(BASELINES, r.label) || r.label;
  for (const r of resized) lines.push(`RESIZED   ${name(r).padEnd(52)} ${r.from} -> ${r.to}`);
  for (const r of moved.sort((a, b) => b.ratio - a.ratio)) {
    lines.push(`MOVED     ${name(r).padEnd(52)} ${r.ratio.toFixed(6)}  ${r.differing}/${r.pixels}`);
  }
  for (const r of noise.sort((a, b) => b.ratio - a.ratio)) {
    lines.push(`noise     ${name(r).padEnd(52)} ${r.ratio.toFixed(6)}  ${r.differing}/${r.pixels}`);
  }
  for (const r of added) lines.push(`ADDED     ${name(r)}`);
  for (const r of deleted) lines.push(`DELETED   ${name(r)}`);
  lines.push("");
  lines.push(
    `${results.length} changed file(s): ${resized.length} resized, ${moved.length} moved past ` +
      `${MAX_DIFF_PIXEL_RATIO}, ${noise.length} within tolerance (rasterisation noise), ` +
      `${added.length} added, ${deleted.length} deleted`,
  );
  if (noise.length && !moved.length && !resized.length) {
    lines.push("Nothing visible changed. The file churn is PNG encoding, not pixels.");
  }
  return lines.join("\n") + "\n";
}

if (process.argv[1] && path.resolve(process.argv[1]).endsWith("baseline-drift.mjs")) main();
