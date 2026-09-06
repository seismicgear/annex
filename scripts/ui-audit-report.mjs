#!/usr/bin/env node
/**
 * ui-audit-report.mjs — Render the UI audit contact sheet.
 *
 * Reads the machine-readable ledger written by `client/e2e/audit/capture.spec.ts`
 * and the captured baselines, and emits `docs/ui-audit/index.html`: every
 * surface, in journey order, with its screenshot and its findings.
 *
 * The point of the contact sheet is reviewability. A JSON ledger tells you
 * there are N accessibility violations; the contact sheet lets a human look at
 * the screen they are on and judge whether the fix landed.
 */

import { existsSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const REPO_ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const AUDIT_ROOT = path.join(REPO_ROOT, 'client', 'e2e', 'audit');
const BASELINE_DIR = path.join(AUDIT_ROOT, 'baselines');
// `reportOnly` surfaces are captured but never diffed, so they are not
// baselines and are not tracked. The contact sheet still shows them.
const CAPTURE_DIR = path.join(AUDIT_ROOT, 'captures');
const DOCS_DIR = path.join(REPO_ROOT, 'docs', 'ui-audit');
const LEDGER_JSONL = path.join(DOCS_DIR, 'findings.jsonl');
const LEDGER_JSON = path.join(DOCS_DIR, 'findings.json');
// One line per surface the capture run STARTED, written by capture.spec.ts.
// Without it this script had no idea what the run in front of it had done: it
// counted the tracked baselines directory, which no run clears, so a run that
// captured nothing announced "103 surfaces captured · 0 findings" — the exact
// phrase this project reads as proof of health — over last run's pictures.
const CAPTURED_JSONL = path.join(DOCS_DIR, 'captured.jsonl');

const SEVERITY_ORDER = { p1: 0, p2: 1, p3: 2 };

/**
 * Read the append-only ledger the capture run produced and consolidate it.
 *
 * The runner appends one JSON object per line as findings are produced,
 * because Playwright restarts its worker process after certain failures and
 * anything held in memory is lost with it — which silently emptied the ledger
 * on exactly the runs that had findings worth reading.
 */
function loadLedger() {
  if (!existsSync(LEDGER_JSONL)) {
    // A clean run with nothing to report writes no lines at all.
    console.warn(`[ui-audit-report] no findings recorded at ${LEDGER_JSONL}`);
    return [];
  }
  const findings = [];
  for (const [i, line] of readFileSync(LEDGER_JSONL, 'utf8').split('\n').entries()) {
    if (!line.trim()) continue;
    try {
      findings.push(JSON.parse(line));
    } catch {
      console.warn(`[ui-audit-report] skipping malformed ledger line ${i + 1}`);
    }
  }
  return findings;
}

/**
 * The surface ids this run exercised.
 *
 * Recorded when a surface STARTS, not when its screenshot succeeds: a run
 * whose surfaces mostly failed is the run whose findings matter most, and it
 * must not be mistaken for a partial one.
 */
function loadCaptured() {
  const ids = new Set();
  if (!existsSync(CAPTURED_JSONL)) return ids;
  for (const line of readFileSync(CAPTURED_JSONL, 'utf8').split('\n')) {
    if (!line.trim()) continue;
    try {
      const id = JSON.parse(line).surfaceId;
      if (id) ids.add(id);
    } catch {
      /* a torn last line from a killed run; the count is a floor either way */
    }
  }
  return ids;
}

function writeConsolidated(findings) {
  const rank = SEVERITY_ORDER;
  const sorted = [...findings].sort(
    (a, b) =>
      String(a.stage).localeCompare(String(b.stage)) ||
      String(a.surfaceId).localeCompare(String(b.surfaceId)) ||
      String(a.viewport).localeCompare(String(b.viewport)) ||
      (rank[a.severity] ?? 9) - (rank[b.severity] ?? 9) ||
      String(a.rule).localeCompare(String(b.rule)),
  );
  writeFileSync(
    LEDGER_JSON,
    `${JSON.stringify({ generatedBy: 'scripts/ui-audit-report.mjs', findings: sorted }, null, 2)}\n`,
  );
  return sorted;
}

/** Discover captured shots as { surfaceId: { viewport: relPathFromDocs } }. */
function loadShots() {
  const shots = {};
  // Approved baselines first, then the report-only captures. Order matters
  // only in that a surface cannot be both, so they never collide.
  for (const root of [BASELINE_DIR, CAPTURE_DIR]) {
    if (!existsSync(root)) continue;
    for (const viewport of readdirSync(root)) {
      const dir = path.join(root, viewport);
      for (const file of readdirSync(dir)) {
        if (!file.endsWith('.png')) continue;
        const id = file.replace(/\.png$/, '');
        shots[id] ??= {};
        shots[id][viewport] = path.relative(DOCS_DIR, path.join(dir, file));
      }
    }
  }
  return shots;
}

const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

function render(findings, shots, captured) {
  const byStage = new Map();
  const stageOf = new Map();
  for (const f of findings) stageOf.set(f.surfaceId, f.stage);

  for (const id of Object.keys(shots)) {
    const stage = stageOf.get(id) ?? 'uncategorised';
    if (!byStage.has(stage)) byStage.set(stage, new Map());
    byStage.get(stage).set(id, []);
  }
  for (const f of findings) {
    if (!byStage.has(f.stage)) byStage.set(f.stage, new Map());
    const surfaces = byStage.get(f.stage);
    if (!surfaces.has(f.surfaceId)) surfaces.set(f.surfaceId, []);
    surfaces.get(f.surfaceId).push(f);
  }

  const counts = { p1: 0, p2: 0, p3: 0 };
  for (const f of findings) counts[f.severity]++;

  const total = Object.keys(shots).length;
  const runBanner =
    captured.size === 0
      ? `<p class="run-warning">This run captured nothing — every picture below is a baseline from an earlier run, and the finding count says nothing about the current tree.</p>`
      : captured.size < total
        ? `<p class="run-warning">Partial run: ${captured.size} of ${total} surfaces were exercised. The rest are baselines from an earlier run.</p>`
        : '';

  const stages = [...byStage.keys()].sort();

  const sections = stages
    .map((stage) => {
      const surfaces = [...byStage.get(stage).entries()].sort(([a], [b]) => a.localeCompare(b));
      const cards = surfaces
        .map(([id, fs]) => {
          const shotSet = shots[id] ?? {};
          const viewports = Object.keys(shotSet).sort();
          const imgs = viewports
            .map(
              (v) =>
                `<figure><figcaption>${esc(v)}</figcaption><a href="${esc(shotSet[v])}"><img loading="lazy" src="${esc(shotSet[v])}" alt="${esc(id)} at ${esc(v)}"></a></figure>`,
            )
            .join('');

          const unique = new Map();
          for (const f of fs) {
            const key = `${f.audit}|${f.rule}|${f.detail}`;
            if (!unique.has(key)) unique.set(key, { ...f, viewports: new Set() });
            unique.get(key).viewports.add(f.viewport);
          }
          const rows = [...unique.values()]
            .sort((a, b) => SEVERITY_ORDER[a.severity] - SEVERITY_ORDER[b.severity])
            .map(
              (f) =>
                `<tr class="sev-${esc(f.severity)}"><td><span class="pill ${esc(f.severity)}">${esc(f.severity.toUpperCase())}</span></td><td>${esc(f.audit)}</td><td><code>${esc(f.rule)}</code></td><td>${esc(f.detail)}${f.target ? `<br><code class="target">${esc(f.target)}</code>` : ''}</td><td class="vp">${[...f.viewports].sort().join(', ')}</td></tr>`,
            )
            .join('');

          const findingsBlock = rows
            ? `<table><thead><tr><th></th><th>Audit</th><th>Rule</th><th>Detail</th><th>Viewports</th></tr></thead><tbody>${rows}</tbody></table>`
            : `<p class="clean">No findings.</p>`;

          // A surface this run never reached still shows its baseline — that
          // is the current picture of it — but it is labelled, so nothing on
          // this page reads as evidence the run did not produce.
          const stale = captured.size > 0 && !captured.has(id);
          const staleNote = stale
            ? `<p class="stale-note">Not exercised in this run — baseline from an earlier one.</p>`
            : '';
          return `<section class="surface${stale ? ' stale' : ''}" id="${esc(id)}"><h3>${esc(id)}</h3>${staleNote}<div class="shots">${imgs}</div>${findingsBlock}</section>`;
        })
        .join('');
      return `<section class="stage"><h2>${esc(stage)}</h2>${cards}</section>`;
    })
    .join('');

  return `<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Annex UI Audit</title>
<style>
  :root { color-scheme: dark light; --bg:#0b0b0c; --fg:#e8e8ea; --muted:#9a9aa2; --line:#26262b; --card:#141416; }
  @media (prefers-color-scheme: light) { :root { --bg:#fff; --fg:#16161a; --muted:#5c5c66; --line:#e3e3e8; --card:#fafafb; } }
  body { margin:0; padding:2rem clamp(1rem,4vw,3rem); background:var(--bg); color:var(--fg);
         font:15px/1.55 ui-sans-serif,system-ui,-apple-system,Segoe UI,Inter,sans-serif; }
  h1 { margin:0 0 .25rem; font-size:1.6rem; }
  .sub { color:var(--muted); margin:0 0 2rem; }
  .totals { display:flex; gap:.5rem; flex-wrap:wrap; margin-bottom:2rem; }
  .run-warning { margin:0 0 1rem; padding:.6rem .8rem; border-left:3px solid #e0a33c;
    background:rgba(224,163,60,.12); color:#e8c489; font-size:.9rem; }
  .surface.stale { opacity:.72; }
  .stale-note { margin:.2rem 0 .6rem; font-size:.78rem; color:#e8c489; }
  .pill { display:inline-block; padding:.15rem .5rem; border-radius:999px; font-size:.72rem; font-weight:600; letter-spacing:.03em; }
  .pill.p1 { background:#7f1d1d; color:#fee2e2; } .pill.p2 { background:#78350f; color:#fef3c7; } .pill.p3 { background:#334155; color:#e2e8f0; }
  .stage { margin:0 0 3rem; } .stage > h2 { font-size:1.05rem; text-transform:uppercase; letter-spacing:.08em; color:var(--muted); border-bottom:1px solid var(--line); padding-bottom:.5rem; }
  .surface { background:var(--card); border:1px solid var(--line); border-radius:10px; padding:1rem 1.25rem; margin:1.25rem 0; }
  .surface h3 { margin:0 0 .75rem; font-size:1rem; font-family:ui-monospace,SFMono-Regular,Menlo,monospace; }
  .shots { display:flex; gap:1rem; overflow-x:auto; padding-bottom:.5rem; }
  figure { margin:0; flex:0 0 auto; } figcaption { font-size:.72rem; color:var(--muted); margin-bottom:.3rem; }
  img { max-height:260px; border:1px solid var(--line); border-radius:6px; display:block; }
  table { width:100%; border-collapse:collapse; margin-top:1rem; font-size:.83rem; }
  th { text-align:left; color:var(--muted); font-weight:600; border-bottom:1px solid var(--line); padding:.35rem .5rem; }
  td { padding:.35rem .5rem; border-bottom:1px solid var(--line); vertical-align:top; }
  code { font-family:ui-monospace,SFMono-Regular,Menlo,monospace; font-size:.92em; }
  code.target { color:var(--muted); }
  .vp { color:var(--muted); white-space:nowrap; }
  .clean { color:#4ade80; font-size:.85rem; margin:.75rem 0 0; }
</style></head><body>
<h1>Annex UI Audit</h1>
<p class="sub">${captured.size} of ${Object.keys(shots).length} surfaces exercised in this run · ${findings.length} findings</p>
${runBanner}
<div class="totals">
  <span class="pill p1">P1 ${counts.p1}</span>
  <span class="pill p2">P2 ${counts.p2}</span>
  <span class="pill p3">P3 ${counts.p3}</span>
</div>
${sections}
</body></html>
`;
}

const rawFindings = loadLedger();
const shots = loadShots();
const captured = loadCaptured();
mkdirSync(DOCS_DIR, { recursive: true });

// `findings.json` is TRACKED — it is the reviewable record of a full sweep.
// Only a full run may rewrite it. A partial run (`--grep`) would narrow it to
// its subset and a run that captured nothing would empty it, and in both cases
// the result looks exactly like a clean full sweep in `git diff`.
const isFullRun = captured.size > 0 && captured.size >= Object.keys(shots).length;
const findings = isFullRun ? writeConsolidated(rawFindings) : rawFindings;
if (!isFullRun) {
  console.warn(
    `[ui-audit-report] ${captured.size} of ${Object.keys(shots).length} surfaces exercised — leaving the tracked findings.json alone`,
  );
}

writeFileSync(path.join(DOCS_DIR, 'index.html'), render(findings, shots, captured));
console.log(
  `[ui-audit-report] ${captured.size} of ${Object.keys(shots).length} surfaces exercised, ${findings.length} findings → docs/ui-audit/index.html`,
);
