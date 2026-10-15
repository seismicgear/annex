#!/usr/bin/env node
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const key = p => `${p.name}@${p.version}`;

// The audit's maintenance dispositions are valid only for the graph reviewed.
// A version, source, feature, parent, expiry, or ignore-list change fails closed.
export function verifyMaintenance({ review, metadata, lock, deny, now = new Date() }) {
  assert.equal(review.schema, 1, 'unsupported maintenance review schema');
  assert.match(review.expires, /^\d{4}-\d{2}-\d{2}$/, 'invalid review expiry');
  assert.ok(now < new Date(`${review.expires}T00:00:00Z`), 'dependency maintenance review expired');
  const configured = deny.match(/^ignore\s*=\s*\[([\s\S]*?)\]/m)?.[1];
  assert.ok(configured, 'advisory ignore list missing');
  // This intentionally accepts only the documented structured entry format.
  // Extracting ids alone would silently overlook valid TOML shorthand strings.
  const ignores = [];
  let remaining = configured.trim();
  while (remaining) {
    const entry = remaining.match(/^\{\s*id\s*=\s*"(RUSTSEC-\d{4}-\d+)"\s*,\s*reason\s*=\s*("(?:[^"\\\r\n]|\\.)*")\s*\}\s*(?:,|$)/);
    assert.ok(entry, 'unrecognized advisory disposition format');
    assert.ok(JSON.parse(entry[2]).trim(), 'advisory disposition reason is empty');
    ignores.push(entry[1]);
    remaining = remaining.slice(entry[0].length).trim();
  }
  assert.deepEqual(ignores.sort(), review.entries.map(e => e.id).sort(), 'unreviewed or missing advisory disposition');
  const packages = new Map(metadata.packages.map(p => [p.id, p]));
  const nodes = new Map(metadata.resolve.nodes.map(n => [n.id, n]));
  const locked = lock.split('[[package]]').slice(1).map(block => {
    const fields = Object.fromEntries([...block.matchAll(/^(name|version|source|checksum)\s*=\s*"([^"]+)"/gm)].map(m => [m[1], m[2]]));
    return fields;
  });
  for (const entry of review.entries) {
    const matches = [...packages.values()].filter(p => p.name === entry.name);
    assert.equal(matches.length, 1, `${entry.name}: reviewed package missing or multiple versions present`);
    const pkg = matches[0];
    assert.equal(pkg.version, entry.version, `${entry.name}: unreviewed version`);
    assert.equal(pkg.source, entry.source, `${entry.name}: unreviewed source`);
    const pin = locked.find(p => key(p) === key(entry));
    assert.ok(pin, `${entry.name}: lock pin missing`);
    assert.equal(pin.source, entry.source, `${entry.name}: lock source changed`);
    assert.equal(pin.checksum, entry.checksum, `${entry.name}: archive checksum changed`);
    const node = nodes.get(pkg.id);
    assert.ok(node, `${entry.name}: resolved package missing`);
    assert.deepEqual([...node.features].sort(), entry.features, `${entry.name}: unreviewed feature exposure`);
    const parents = [...nodes.values()].filter(n => n.dependencies.includes(pkg.id)).map(n => key(packages.get(n.id))).sort();
    assert.deepEqual(parents, entry.parents.map(key).sort(), `${entry.name}: unreviewed parent exposure`);
    if (['derivative', 'paste'].includes(entry.name)) {
      assert.ok(pkg.targets.some(t => t.kind.includes('proc-macro')), `${entry.name}: expected compile-time procedural macro`);
    }
    if (entry.name === 'proc-macro-error') {
      for (const parent of [...nodes.values()].filter(n => n.dependencies.includes(pkg.id))) {
        assert.ok(packages.get(parent.id).targets.some(t => t.kind.includes('proc-macro')), 'proc-macro-error: runtime parent is not reviewed');
      }
    }
  }
  return review.entries.length;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const args = process.argv.slice(2);
    assert.ok(args.length === 2 && args[0] === '--metadata', 'Usage: node scripts/verify-dependency-maintenance.mjs --metadata <locked-all-features-metadata.json>');
    const [review, metadata, lock, deny] = await Promise.all([
      readFile(path.join(root, 'scripts/dependency-maintenance-review.json'), 'utf8').then(JSON.parse),
      readFile(path.resolve(args[1]), 'utf8').then(s => JSON.parse(s.replace(/^\uFEFF/, ''))),
      readFile(path.join(root, 'Cargo.lock'), 'utf8'),
      readFile(path.join(root, 'deny.toml'), 'utf8'),
    ]);
    const count = verifyMaintenance({ review, metadata, lock, deny });
    console.log(`Verified ${count} exact maintenance dispositions; review expires ${review.expires}.`);
  } catch (error) {
    console.error(`[dependency-maintenance] ${error.message}`);
    process.exitCode = 1;
  }
}
