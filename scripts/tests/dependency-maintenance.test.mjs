import test from 'node:test';
import assert from 'node:assert/strict';
import { verifyMaintenance } from '../verify-dependency-maintenance.mjs';

function fixture() {
  return {
    review: { schema: 1, expires: '2027-01-13', entries: [{ id: 'RUSTSEC-2025-0141', name: 'bincode', version: '1.3.3', source: 'registry+https://example.invalid/index', checksum: 'archive-digest', features: [], parents: [{ name: 'webrtc-dtls', version: '0.10.0' }] }] },
    metadata: {
      packages: [{ id: 'child', name: 'bincode', version: '1.3.3', source: 'registry+https://example.invalid/index', targets: [{ kind: ['lib'] }] }, { id: 'parent', name: 'webrtc-dtls', version: '0.10.0', targets: [{ kind: ['lib'] }] }],
      resolve: { nodes: [{ id: 'child', features: [], dependencies: [] }, { id: 'parent', features: [], dependencies: ['child'] }] },
    },
    lock: '[[package]]\nname = "bincode"\nversion = "1.3.3"\nsource = "registry+https://example.invalid/index"\nchecksum = "archive-digest"\n',
    deny: 'ignore = [{ id = "RUSTSEC-2025-0141", reason = "reviewed maintenance" }]',
    now: new Date('2026-10-15T00:00:00Z'),
  };
}

test('accepts only the exact reviewed package graph before expiry', () => assert.equal(verifyMaintenance(fixture()), 1));
for (const [name, mutate, expected] of [
  ['expired review', f => { f.now = new Date('2027-01-13T00:00:00Z'); }, /expired/],
  ['changed version', f => { f.metadata.packages[0].version = '1.3.4'; }, /unreviewed version/],
  ['changed source', f => { f.metadata.packages[0].source = null; }, /unreviewed source/],
  ['changed checksum', f => { f.lock = f.lock.replace('archive-digest', 'other'); }, /checksum/],
  ['new feature exposure', f => { f.metadata.resolve.nodes[0].features.push('new-parser'); }, /feature exposure/],
  ['changed direct parent', f => { f.metadata.packages[1].version = '0.12.0'; }, /parent exposure/],
  ['missing package', f => { f.metadata.packages.shift(); }, /missing or multiple/],
  ['new ignored advisory', f => { f.deny = f.deny.replace('ignore = [', 'ignore = [{ id = "RUSTSEC-2026-9999", reason = "unreviewed" },'); }, /unreviewed or missing/],
  ['shorthand ignored advisory', f => { f.deny = f.deny.replace('ignore = [', 'ignore = ["RUSTSEC-2026-9999",'); }, /unrecognized/],
  ['unrecognized disposition field', f => { f.deny = f.deny.replace('reason = "reviewed maintenance"', 'reason = "reviewed maintenance", extra = true'); }, /unrecognized/],
  ['empty disposition reason', f => { f.deny = f.deny.replace('reviewed maintenance', ''); }, /reason is empty/],
]) {
  test(`rejects ${name}`, () => { const f = fixture(); mutate(f); assert.throws(() => verifyMaintenance(f), expected); });
}
