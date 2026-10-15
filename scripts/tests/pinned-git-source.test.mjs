import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { preparePinnedGitSource } from '../pinned-git-source.mjs';
import { whisperBuildDirectory } from '../prepare-desktop-assets.mjs';

function git(cwd, ...args) {
  const result = spawnSync('git', args, { cwd, encoding: 'utf8', windowsHide: true });
  if (result.error) throw result.error;
  assert.equal(result.status, 0, result.stderr);
  return result.stdout.trim();
}
async function fixture(t) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'annex-source-pin-'));
  t.after(async () => {
    const resolved = await fs.realpath(root);
    assert.ok(path.basename(resolved).startsWith('annex-source-pin-'));
    assert.equal(path.dirname(resolved), await fs.realpath(os.tmpdir()));
    await fs.rm(resolved, { recursive: true, force: true });
  });
  const upstream = path.join(root, 'upstream');
  const parent = path.join(root, 'annex');
  const cache = path.join(parent, 'target', 'assets');
  for (const dir of [upstream, parent]) {
    await fs.mkdir(dir); git(dir, 'init');
    git(dir, 'config', 'user.name', 'Annex source regression');
    git(dir, 'config', 'user.email', 'source-regression@invalid');
  }
  await fs.writeFile(path.join(upstream, 'source.txt'), 'pinned upstream bytes\n');
  await fs.writeFile(path.join(upstream, 'CMakeLists.txt'), 'cmake_minimum_required(VERSION 3.15)\nproject(AnnexSourceCacheRegression LANGUAGES NONE)\n');
  await fs.writeFile(path.join(upstream, '.gitattributes'), 'source.txt -text\n');
  git(upstream, 'add', 'source.txt', 'CMakeLists.txt', '.gitattributes'); git(upstream, 'commit', '-m', 'pinned source'); git(upstream, 'tag', 'v-test');
  await fs.writeFile(path.join(parent, 'parent.txt'), 'Annex ancestor must never be used\n');
  git(parent, 'add', 'parent.txt'); git(parent, 'commit', '-m', 'different parent commit');
  await fs.mkdir(cache, { recursive: true });
  return { root, upstream, parent, cache, options: { cache, name: 'whisper-source', repository: upstream, tag: 'v-test', commit: git(upstream, 'rev-parse', 'HEAD') } };
}

test('fresh source is a real independent checkout at the exact pin', async t => {
  const { options, parent } = await fixture(t);
  const source = await preparePinnedGitSource(options);
  assert.equal(git(source, 'rev-parse', 'HEAD'), options.commit);
  assert.notEqual(git(parent, 'rev-parse', 'HEAD'), options.commit);
  assert.equal(await fs.realpath(git(source, 'rev-parse', '--show-toplevel')), source);
  assert.equal(await fs.readFile(path.join(source, 'source.txt'), 'utf8'), 'pinned upstream bytes\n');
});

test('valid cached source is reused without contacting the repository', async t => {
  const { options } = await fixture(t);
  const first = await preparePinnedGitSource(options);
  const second = await preparePinnedGitSource({ ...options, repository: path.join(options.cache, 'nonexistent-upstream') });
  assert.equal(second, first);
});

test('restored files without their own Git metadata never accept an ancestor HEAD and are preserved', async t => {
  const { options, parent } = await fixture(t);
  const stale = path.join(options.cache, options.name);
  await fs.mkdir(stale);
  await fs.writeFile(path.join(stale, 'source.txt'), 'stale modified cache bytes');
  // This reproduces the exact previous bug: ordinary Git walks up to Annex.
  assert.equal(git(stale, 'rev-parse', 'HEAD'), git(parent, 'rev-parse', 'HEAD'));
  const fresh = await preparePinnedGitSource(options);
  assert.notEqual(fresh, await fs.realpath(stale));
  assert.equal(await fs.readFile(path.join(stale, 'source.txt'), 'utf8'), 'stale modified cache bytes');
  assert.equal(git(fresh, 'rev-parse', 'HEAD'), options.commit);
  assert.equal(await fs.readFile(path.join(fresh, 'source.txt'), 'utf8'), 'pinned upstream bytes\n');
  assert.equal(await preparePinnedGitSource({ ...options, repository: path.join(options.cache, 'nonexistent-upstream') }), fresh);
});

test('a genuine independent checkout at the wrong pin is rejected rather than replaced', async t => {
  const { options, parent } = await fixture(t);
  const source = await preparePinnedGitSource(options);
  await assert.rejects(preparePinnedGitSource({ ...options, commit: git(parent, 'rev-parse', 'HEAD') }), /differs from pin/);
  assert.equal(git(source, 'rev-parse', 'HEAD'), options.commit);
});

test('modified tracked source and untracked build inputs both fail the clean-source gate', async t => {
  const { options } = await fixture(t);
  const source = await preparePinnedGitSource(options);
  await fs.writeFile(path.join(source, 'source.txt'), 'modified');
  await assert.rejects(preparePinnedGitSource(options), /not clean/);
  git(source, 'restore', 'source.txt');
  await fs.writeFile(path.join(source, 'untracked.cmake'), 'changed build input');
  await assert.rejects(preparePinnedGitSource(options), /not clean/);
});

test('a Git file pointing elsewhere is rejected rather than treated as owned metadata', async t => {
  const { options } = await fixture(t);
  const source = path.join(options.cache, options.name);
  await fs.mkdir(source);
  await fs.writeFile(path.join(source, '.git'), `gitdir: ${path.join(options.repository, '.git')}\n`);
  await assert.rejects(preparePinnedGitSource(options), /own an ordinary .git directory/);
});

test('recovered source uses a separate real CMake tree and preserves the old configured build', async t => {
  const { options } = await fixture(t);
  const stale = await preparePinnedGitSource(options);
  const oldBuild = path.join(options.cache, 'whisper-build');
  const configure = (source, build) => {
    const result = spawnSync('cmake', ['-S', source, '-B', build], { encoding: 'utf8', windowsHide: true });
    if (result.error) throw result.error;
    assert.equal(result.status, 0, result.stdout + result.stderr);
  };
  configure(stale, oldBuild);
  const oldCache = await fs.readFile(path.join(oldBuild, 'CMakeCache.txt'));
  await fs.rename(path.join(stale, '.git'), path.join(options.cache, 'preserved-original-git'));
  const fresh = await preparePinnedGitSource(options);
  const freshBuild = whisperBuildDirectory(options.cache, fresh);
  assert.notEqual(freshBuild, oldBuild);
  assert.notEqual(freshBuild, whisperBuildDirectory(options.cache, stale));
  assert.notEqual(freshBuild, whisperBuildDirectory(options.cache, fresh, 'Ninja'));
  configure(fresh, freshBuild);
  assert.deepEqual(await fs.readFile(path.join(oldBuild, 'CMakeCache.txt')), oldCache);
  const newCache = await fs.readFile(path.join(freshBuild, 'CMakeCache.txt'), 'utf8');
  const home = newCache.match(/^CMAKE_HOME_DIRECTORY:INTERNAL=(.+)$/m)?.[1].trim();
  assert.equal(await fs.realpath(home), fresh);
});
