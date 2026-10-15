import fs from 'node:fs/promises';
import path from 'node:path';
import { spawnSync } from 'node:child_process';

function git(args, cwd) {
  const result = spawnSync('git', args, { cwd, windowsHide: true, encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 32 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`Pinned source Git command failed (${result.status}): ${result.stderr}`);
  return result.stdout.trim();
}

async function statOrMissing(filename) {
  try { return await fs.lstat(filename); }
  catch (error) { if (error.code === 'ENOENT') return undefined; throw error; }
}

async function inspectSource(source) {
  const present = await statOrMissing(source);
  if (present && (!present.isDirectory() || present.isSymbolicLink())) {
    throw new Error(`Pinned source cache is not an ordinary directory: ${source}`);
  }
  const metadata = present && await statOrMissing(path.join(source, '.git'));
  if (metadata && (!metadata.isDirectory() || metadata.isSymbolicLink())) {
    throw new Error(`Pinned source must own an ordinary .git directory: ${source}`);
  }
  return { present, metadata };
}

export async function preparePinnedGitSource({ cache, name, repository, tag, commit }) {
  if (!/^[a-z0-9][a-z0-9-]*$/.test(name) || !/^[0-9a-f]{40}$/.test(commit)) {
    throw new Error('Pinned source requires an exact source name and immutable Git commit');
  }
  const root = await fs.realpath(cache);
  let source = path.join(root, name);
  let { present, metadata } = await inspectSource(source);
  if (present && !metadata) {
    // Reuse a previous verified replacement without cloning on every invocation.
    source = path.join(root, `${name}-pinned-${commit}`);
    ({ present, metadata } = await inspectSource(source));
  }
  if (!metadata) {
    // Cargo caches can restore nested source files without their .git metadata.
    // Preserve that incomplete cache; a new clone must establish the source pin.
    if (present) source = await fs.mkdtemp(path.join(root, `${name}-fresh-`));
    git(['clone', '--depth', '1', '--branch', tag, '--', repository, source], root);
  }
  const ownMetadata = await statOrMissing(path.join(source, '.git'));
  if (!ownMetadata?.isDirectory() || ownMetadata.isSymbolicLink()) {
    throw new Error(`Pinned source clone has no independent .git directory: ${source}`);
  }
  const canonical = await fs.realpath(source);
  if (!canonical.startsWith(root + path.sep)) throw new Error('Pinned source escapes its cache');
  // Explicit paths prevent Git from walking up to the Annex repository when a
  // nested checkout is incomplete, even when its parent has a valid HEAD.
  const args = ['--git-dir', path.join(canonical, '.git'), '--work-tree', canonical];
  const reportedRoot = await fs.realpath(git([...args, 'rev-parse', '--show-toplevel'], canonical));
  if (reportedRoot !== canonical) throw new Error(`Pinned source Git root differs: ${canonical}`);
  const actual = git([...args, 'rev-parse', 'HEAD'], canonical);
  if (actual !== commit) throw new Error(`Pinned source commit ${actual} differs from pin ${commit}: ${canonical}`);
  const changed = git([...args, 'status', '--porcelain', '--untracked-files=normal'], canonical);
  if (changed) throw new Error(`Pinned source is not clean at commit ${actual}: ${canonical}\n${changed}`);
  return canonical;
}
