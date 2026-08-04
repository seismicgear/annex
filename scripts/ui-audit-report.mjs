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
const DOCS_DIR = path.join(REPO_ROOT, 'docs', 'ui-audit');
const LEDGER = path.join(DOCS_DIR, 'findings.json');

const SEVERITY_ORDER = { p1: 0, p2: 1, p3: 2 };

function loadLedger() {
  if (!existsSync(LEDGER)) {
    console.error(`[ui-audit-report] no ledger at ${LEDGER} — run scripts/ui-audit.sh first`);
    process.exit(1);
  }
  return JSON.parse(readFileSync(LEDGER, 'utf8')).findings ?? [];
}

/** Discover captured shots as { surfaceId: { viewport: relPathFromDocs } }. */
function loadShots() {
  const shots = {};
  if (!existsSync(BASELINE_DIR)) return shots;
  for (const viewport of readdirSync(BASELINE_DIR)) {
    const dir = path.join(BASELINE_DIR, viewport);
    for (const file of readdirSync(dir)) {
      if (!file.endsWith('.png')) continue;
      const id = file.replace(/\.png$/, '');
      shots[id] ??= {};
      shots[id][viewport] = path.relative(DOCS_DIR, path.join(dir, file));
    }
  }
  return shots;
}

const esc = (s) =>
  String(s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);

function render(findings, shots) {
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

          return `<section class="surface" id="${esc(id)}"><h3>${esc(id)}</h3><div class="shots">${imgs}</div>${findingsBlock}</section>`;
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
<p class="sub">${Object.keys(shots).length} surfaces captured · ${findings.length} findings</p>
<div class="totals">
  <span class="pill p1">P1 ${counts.p1}</span>
  <span class="pill p2">P2 ${counts.p2}</span>
  <span class="pill p3">P3 ${counts.p3}</span>
</div>
${sections}
</body></html>
`;
}

const findings = loadLedger();
const shots = loadShots();
mkdirSync(DOCS_DIR, { recursive: true });
writeFileSync(path.join(DOCS_DIR, 'index.html'), render(findings, shots));
console.log(
  `[ui-audit-report] ${Object.keys(shots).length} surfaces, ${findings.length} findings → docs/ui-audit/index.html`,
);
