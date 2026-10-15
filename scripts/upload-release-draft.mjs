#!/usr/bin/env node
// Reuse a verified draft by immutable ID. The tag endpoint may omit drafts;
// publication is a separate operator action and is never performed here.
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import {createReadStream} from 'node:fs';
import {createHash} from 'node:crypto';
import path from 'node:path';
import {fileURLToPath} from 'node:url';
import {checkReleaseWriteEligibility} from './check-release-write-eligibility.mjs';

const hashFile = async filename => {
  const hash = createHash('sha256');
  for await (const bytes of createReadStream(filename)) hash.update(bytes);
  return hash.digest('hex');
};

export async function uploadReleaseDraft({repository, tag, commit, token, directory, notes,
  fetchImpl = fetch}) {
  const entries = await fs.readdir(directory);
  assert.ok(entries.includes('SHA256SUMS') && entries.includes('downloads.json'));
  const sums = new Map();
  for (const line of (await fs.readFile(path.join(directory, 'SHA256SUMS'), 'utf8')).trim().split(/\r?\n/)) {
    const match = /^([a-f0-9]{64})  ([A-Za-z0-9_.+-]+)$/.exec(line);
    assert.ok(match && !sums.has(match[2]), 'Malformed or duplicate release checksum');
    sums.set(match[2], match[1]);
  }
  assert.deepEqual([...sums.keys()].sort(), entries.filter(name => name !== 'SHA256SUMS').sort());
  const inventory = [];
  for (const name of entries.sort()) {
    assert.match(name, /^[A-Za-z0-9_.+-]+$/);
    const filename = path.join(directory, name), stat = await fs.lstat(filename);
    assert.ok(stat.isFile() && !stat.isSymbolicLink());
    const sha256 = await hashFile(filename);
    if (name !== 'SHA256SUMS') assert.equal(sha256, sums.get(name), `Changed release file: ${name}`);
    inventory.push({name, filename, size: stat.size, digest: `sha256:${sha256}`});
  }
  const catalog = JSON.parse(await fs.readFile(path.join(directory, 'downloads.json'), 'utf8'));
  assert.equal(catalog.tag, tag); assert.equal(catalog.sourceCommit, commit);
  const body = await fs.readFile(notes, 'utf8');
  assert.ok(body.trim(), 'Release notes are required');
  const eligibility = await checkReleaseWriteEligibility({repository, tag, commit, token, fetchImpl});
  const base = `https://api.github.com/repos/${repository}/releases`;
  async function request(url, options = {}) {
    let response;
    try {
      response = await fetchImpl(url, {...options, redirect: 'error',
        signal: AbortSignal.timeout(options.duplex ? 900_000 : 30_000),
        headers: {Accept: 'application/vnd.github+json', Authorization: `Bearer ${token}`,
          'X-GitHub-Api-Version': '2022-11-28', 'User-Agent': 'annex-release-draft-upload',
          ...options.headers}});
    } catch { throw new Error('Release draft request failed; publication was not attempted.'); }
    if (!response.ok) throw new Error(`Release draft request failed (HTTP ${response.status}); no asset overwrite or publication is permitted.`);
    return response.json();
  }
  function checkDraft(release, id) {
    assert.ok(Number.isSafeInteger(release.id) && release.id > 0);
    if (id !== undefined) assert.equal(release.id, id);
    assert.equal(release.tag_name, tag); assert.equal(release.target_commitish, commit);
    assert.equal(release.draft, true); assert.equal(release.published_at, null);
    assert.ok(Array.isArray(release.assets));
    assert.equal(new Set(release.assets.map(asset => asset.name)).size, release.assets.length);
    for (const asset of release.assets) {
      const expected = inventory.find(row => row.name === asset.name);
      assert.ok(expected, `Unexpected draft asset: ${asset.name}`);
      assert.equal(asset.state, 'uploaded');
      assert.equal(asset.size, expected.size); assert.equal(asset.digest, expected.digest);
    }
  }
  let release;
  if (eligibility.state === 'absent') {
    release = await request(base, {method: 'POST', headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({tag_name: tag, target_commitish: commit, name: tag, body,
        draft: true, prerelease: false})});
  } else release = await request(`${base}/${eligibility.releaseId}`);
  checkDraft(release, eligibility.releaseId);
  const id = release.id;
  for (const asset of inventory) {
    // Recheck the immutable ID before every mutation, including a concurrent
    // operator publication. Existing files must match; none are overwritten.
    release = await request(`${base}/${id}`); checkDraft(release, id);
    if (release.assets.some(row => row.name === asset.name)) continue;
    const uploaded = await request(`https://uploads.github.com/repos/${repository}/releases/${id}/assets?name=${encodeURIComponent(asset.name)}`,
      {method: 'POST', headers: {'Content-Type': 'application/octet-stream', 'Content-Length': String(asset.size)},
        body: createReadStream(asset.filename), duplex: 'half'});
    assert.equal(uploaded.name, asset.name); assert.equal(uploaded.size, asset.size);
    assert.equal(uploaded.digest, asset.digest); assert.equal(uploaded.state, 'uploaded');
  }
  release = await request(`${base}/${id}`); checkDraft(release, id);
  assert.deepEqual(release.assets.map(asset => asset.name).sort(), entries.sort());
  return {repository, tag, commit, releaseId: id, draft: true, published: false,
    assets: release.assets.map(({id, name, size, digest}) => ({id, name, size, digest}))};
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const receipt = await uploadReleaseDraft({repository: process.env.GITHUB_REPOSITORY,
      tag: process.env.ANNEX_RELEASE_TAG, commit: process.env.GITHUB_SHA, token: process.env.GH_TOKEN,
      directory: 'release-assets', notes: 'RELEASE_NOTES.md'});
    await fs.writeFile('release-draft-upload.json', JSON.stringify(receipt, null, 2) + '\n');
    console.log(JSON.stringify({releaseId: receipt.releaseId, assets: receipt.assets.length, draft: true, published: false}));
  } catch (error) {
    console.error(`[release-draft-upload] ${error.message}`); process.exitCode = 1;
  }
}
