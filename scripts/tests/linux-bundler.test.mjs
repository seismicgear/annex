import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { prepareLinuxBundler } from '../prepare-linux-bundler.mjs';

const production = JSON.parse(await fs.readFile(new URL('../linux-bundler-tools.json', import.meta.url), 'utf8'));
async function fixture(t) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'annex-linux-bundler-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const pins = structuredClone(production);
  const contents = new Map();
  for (const pin of pins.files) {
    const bytes = Buffer.from(`harmless executable fixture for ${pin.name}\n`);
    Object.assign(pin, { bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
    contents.set(pin.name, bytes);
  }
  const toolsDirectory = path.join(root, 'tauri');
  const archiveDirectory = path.join(root, 'verified');
  await fs.mkdir(toolsDirectory);
  await fs.mkdir(archiveDirectory);
  let requests = 0;
  const downloader = async (_url, file, pin) => {
    if (!(await fs.stat(file).catch(() => null))) { requests++; await fs.writeFile(file, contents.get(pin.name)); }
  };
  return { root, pins, contents, toolsDirectory, archiveDirectory, downloader, requests: () => requests };
}

test('a second build restores Tauri-mutated bytes from independently verified originals', async t => {
  const f = await fixture(t);
  await fs.writeFile(path.join(f.toolsDirectory, 'unrelated-cache-file'), 'preserved');
  await prepareLinuxBundler(f);
  const linuxdeploy = path.join(f.toolsDirectory, 'linuxdeploy-x86_64.AppImage');
  const changed = await fs.readFile(linuxdeploy);
  changed.fill(0, 8, 11);
  await fs.writeFile(linuxdeploy, changed);
  await prepareLinuxBundler(f);
  assert.equal(f.requests(), 5);
  for (const pin of f.pins.files) {
    assert.deepEqual(await fs.readFile(path.join(f.toolsDirectory, pin.name)), f.contents.get(pin.name));
    assert.deepEqual(await fs.readFile(path.join(f.archiveDirectory, `${pin.sha256}-${pin.name}`)), f.contents.get(pin.name));
    if (process.platform !== 'win32') assert.equal((await fs.stat(path.join(f.toolsDirectory, pin.name))).mode & 0o777, 0o755);
  }
  assert.equal(await fs.readFile(path.join(f.toolsDirectory, 'unrelated-cache-file'), 'utf8'), 'preserved');
  assert.equal((await fs.readdir(f.toolsDirectory)).some(name => name.startsWith('.annex-linux-stage-')), false);
});

test('corruption of a later download leaves every existing tool untouched', async t => {
  const f = await fixture(t);
  for (const pin of f.pins.files) await fs.writeFile(path.join(f.toolsDirectory, pin.name), 'previous executable');
  const last = f.pins.files.at(-1);
  await fs.writeFile(path.join(f.archiveDirectory, `${last.sha256}-${last.name}`), Buffer.alloc(last.bytes));
  await assert.rejects(prepareLinuxBundler(f), /verification failed/);
  for (const pin of f.pins.files) assert.equal(await fs.readFile(path.join(f.toolsDirectory, pin.name), 'utf8'), 'previous executable');
  assert.equal((await fs.readdir(f.toolsDirectory)).some(name => name.startsWith('.annex-linux-stage-')), false);
});

test('unsafe names and overlapping download/cache locations are rejected', async t => {
  const f = await fixture(t);
  const bad = structuredClone(f.pins);
  bad.files[0].name = '../escape';
  await assert.rejects(prepareLinuxBundler({ ...f, pins: bad }), /reviewed Tauri/);
  await assert.rejects(prepareLinuxBundler({ ...f, archiveDirectory: f.toolsDirectory }), /must be separate/);
  await assert.rejects(prepareLinuxBundler({ ...f, archiveDirectory: path.join(f.toolsDirectory, 'nested') }), /must be separate/);
  assert.equal(f.requests(), 0);
});

test('cache links cannot overwrite a file outside the bundler cache', { skip: process.platform === 'win32' }, async t => {
  const f = await fixture(t);
  const outside = path.join(f.root, 'outside');
  await fs.writeFile(outside, 'preserved');
  await fs.symlink(outside, path.join(f.toolsDirectory, f.pins.files[0].name));
  await assert.rejects(prepareLinuxBundler(f), /Unsafe Linux bundler file/);
  assert.equal(await fs.readFile(outside, 'utf8'), 'preserved');
  assert.equal(f.requests(), 0);
});

test('verified download links and linked parent directories are rejected', { skip: process.platform === 'win32' }, async t => {
  const f = await fixture(t);
  const outside = path.join(f.root, 'outside');
  await fs.mkdir(outside);
  const link = path.join(f.root, 'linked-parent');
  await fs.symlink(outside, link);
  await assert.rejects(prepareLinuxBundler({ ...f, toolsDirectory: path.join(link, 'tauri') }), /Unsafe Linux bundler directory/);
  const pin = f.pins.files[0];
  const original = path.join(outside, 'original');
  await fs.writeFile(original, f.contents.get(pin.name));
  await fs.symlink(original, path.join(f.archiveDirectory, `${pin.sha256}-${pin.name}`));
  await assert.rejects(prepareLinuxBundler(f), /Unsafe Linux bundler file/);
  assert.equal(f.requests(), 0);
});
