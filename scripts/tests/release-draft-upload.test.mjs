import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {createHash} from 'node:crypto';
import {uploadReleaseDraft} from '../upload-release-draft.mjs';

const commit = 'a'.repeat(40), tag = `dry-run-${commit}`, repository = 'seismicgear/annex';
const digest = bytes => createHash('sha256').update(bytes).digest('hex');
const json = (status, value) => new Response(JSON.stringify(value), {status});
async function fixture(t, options = {}) {
  const temporary = await fs.mkdtemp(path.join(os.tmpdir(), 'annex-draft-upload-test-'));
  t.after(async () => {
    assert.ok(path.resolve(temporary).startsWith(path.resolve(os.tmpdir()) + path.sep));
    assert.match(path.basename(temporary), /^annex-draft-upload-test-/);
    await fs.rm(temporary, {recursive: true});
  });
  const directory = path.join(temporary, 'assets'); await fs.mkdir(directory);
  const payloads = {
    'downloads.json': JSON.stringify({tag, sourceCommit: commit}),
    'payload.bin': 'unit-test bytes; this is not a release installer',
  };
  payloads.SHA256SUMS = Object.entries(payloads).map(([name, bytes]) => `${digest(bytes)}  ${name}`).join('\n') + '\n';
  for (const [name, bytes] of Object.entries(payloads)) await fs.writeFile(path.join(directory, name), bytes);
  const notes = path.join(temporary, 'notes.md'); await fs.writeFile(notes, 'Unit test release');
  let release = {id: 42, tag_name: tag, target_commitish: commit, draft: true,
    published_at: null, assets: []};
  if (options.complete) release.assets = Object.entries(payloads).map(([name, bytes], index) => ({id: index + 1,
    name, size: Buffer.byteLength(bytes), digest: `sha256:${digest(bytes)}`, state: 'uploaded'}));
  const calls = []; let reads = 0;
  const fetchImpl = async (address, request) => {
    const url = new URL(address); calls.push({address, method: request.method ?? 'GET'});
    assert.equal(request.redirect, 'error');
    assert.equal(request.headers.Authorization, 'Bearer test-token');
    if (url.pathname === `/repos/${repository}`) return json(200, {full_name: repository});
    if (url.pathname.includes('/releases/tags/')) return json(404, {});
    if (url.pathname === `/repos/${repository}/releases` && !request.method)
      return json(200, options.absent ? [] : [release]);
    if (url.pathname === `/repos/${repository}/releases` && request.method === 'POST') {
      const body = JSON.parse(request.body);
      assert.equal(body.draft, true); assert.equal(body.target_commitish, commit);
      assert.equal(body.tag_name, tag);
      if (options.createFailure) return json(403, {message: 'refused'});
      release = {...release, ...body}; return json(201, release);
    }
    if (url.pathname === `/repos/${repository}/releases/42` && !request.method) {
      reads++; options.beforeRead?.(release, reads);
      return json(200, release);
    }
    if (url.host === 'uploads.github.com' && url.pathname.endsWith('/releases/42/assets') && request.method === 'POST') {
      const name = url.searchParams.get('name');
      assert.ok(!release.assets.some(asset => asset.name === name), 'Never replace an existing asset');
      const chunks = []; for await (const chunk of request.body) chunks.push(chunk);
      const bytes = Buffer.concat(chunks); assert.equal(bytes.toString(), payloads[name]);
      assert.equal(request.headers['Content-Length'], String(bytes.length));
      const asset = {id: 100 + release.assets.length, name, size: bytes.length,
        digest: `sha256:${digest(bytes)}`, state: 'uploaded'};
      if (options.corruptUpload) asset.digest = 'sha256:' + 'b'.repeat(64);
      release.assets.push(asset); return json(201, asset);
    }
    throw new Error('Unexpected operation ' + request.method + ' ' + address);
  };
  return {input: {repository, tag, commit, token: 'test-token', directory, notes, fetchImpl}, calls};
}

test('reuses an authenticated untagged draft by ID despite the tag endpoint returning 404', async t => {
  const {input, calls} = await fixture(t);
  const receipt = await uploadReleaseDraft(input);
  assert.equal(receipt.releaseId, 42); assert.equal(receipt.published, false);
  assert.equal(receipt.assets.length, 3);
  assert.equal(calls.filter(call => call.method === 'POST').length, 3);
  assert.ok(calls.filter(call => call.method === 'POST').every(call => call.address.includes('uploads.github.com')));
});
test('identical existing assets are read back without any mutation', async t => {
  const {input, calls} = await fixture(t, {complete: true});
  const receipt = await uploadReleaseDraft(input);
  assert.equal(receipt.assets.length, 3); assert.ok(calls.every(call => call.method === 'GET'));
});
test('an absent release is created only as an exact-source draft', async t => {
  const {input, calls} = await fixture(t, {absent: true});
  assert.equal((await uploadReleaseDraft(input)).published, false);
  assert.equal(calls.filter(call => call.method === 'POST').length, 4);
});
test('creation failure is reported without retry, retagging, or permission escalation', async t => {
  const {input, calls} = await fixture(t, {absent: true, createFailure: true});
  await assert.rejects(uploadReleaseDraft(input), /HTTP 403/);
  assert.equal(calls.filter(call => call.method === 'POST').length, 1);
});
for (const [name, change] of [
  ['publication between eligibility and upload', release => { release.draft = false; release.published_at = '2026-10-15T00:00:00Z'; }],
  ['changed source', release => { release.target_commitish = 'main'; }],
  ['different ID', release => { release.id = 43; }],
  ['unexpected asset', release => { release.assets.push({name: 'unreviewed.exe'}); }],
]) test(`refuses ${name} without uploading`, async t => {
  const {input, calls} = await fixture(t, {beforeRead: change});
  await assert.rejects(uploadReleaseDraft(input));
  assert.ok(calls.every(call => call.method === 'GET'));
});
test('changed existing asset is never overwritten', async t => {
  const {input, calls} = await fixture(t, {complete: true, beforeRead: release => { release.assets[0].digest = 'sha256:' + 'b'.repeat(64); }});
  await assert.rejects(uploadReleaseDraft(input)); assert.ok(calls.every(call => call.method === 'GET'));
});
test('changed local bytes fail before the first network call', async t => {
  const {input, calls} = await fixture(t);
  await fs.appendFile(path.join(input.directory, 'payload.bin'), 'changed');
  await assert.rejects(uploadReleaseDraft(input), /Changed release file/); assert.equal(calls.length, 0);
});
test('incorrect upload digest stops before subsequent assets', async t => {
  const {input, calls} = await fixture(t, {corruptUpload: true});
  await assert.rejects(uploadReleaseDraft(input));
  assert.equal(calls.filter(call => call.method === 'POST').length, 1);
});
