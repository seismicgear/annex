import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { platformFor, checkedPath, verifyFile, cmakeFlags } from '../prepare-desktop-assets.mjs';

test('only matching explicitly supported native architectures can prepare a runtime', () => {
  assert.equal(platformFor('x86_64-pc-windows-msvc', 'win32', 'x64'), 'win32-x64');
  assert.equal(platformFor('aarch64-apple-darwin', 'darwin', 'arm64'), 'darwin-arm64');
  assert.throws(() => platformFor('x86_64-apple-darwin', 'darwin', 'arm64'), /cross-architecture/);
  assert.throws(() => platformFor(undefined, 'linux', 'arm64'), /Unsupported/);
});
test('archive and manifest paths cannot leave their exact resource destination', () => {
  for (const value of ['../secret', '/secret', 'C:/secret', 'piper/../../secret', 'piper\\secret', 'piper/./secret']) {
    assert.throws(() => checkedPath('/tmp/stage', value), /Unsafe/);
  }
  assert.match(checkedPath('/tmp/stage', 'piper/lib/runtime.dll'), /runtime\.dll$/);
});
test('pinned verification rejects both corrupted and truncated existing resources', async () => {
  const folder = await fs.mkdtemp(path.join(os.tmpdir(), 'annex-asset-test-'));
  const file = path.join(folder, 'runtime');
  const bytes = Buffer.from('real bounded digest fixture');
  const pin = { sha256: createHash('sha256').update(bytes).digest('hex'), bytes: bytes.length };
  try {
    await fs.writeFile(file, bytes); await verifyFile(file, pin);
    await fs.writeFile(file, Buffer.alloc(bytes.length)); await assert.rejects(verifyFile(file, pin), /verification failed/);
    await fs.writeFile(file, bytes.subarray(0,-1)); await assert.rejects(verifyFile(file, pin), /verification failed/);
  } finally { await fs.unlink(file); await fs.rmdir(folder); }
});
test('portable Whisper build disables host-specific and unbundled accelerator libraries', () => {
  for (const flag of ['-DBUILD_SHARED_LIBS=OFF', '-DGGML_NATIVE=OFF', '-DGGML_OPENMP=OFF', '-DGGML_METAL=OFF', '-DGGML_BACKEND_DL=OFF']) assert.ok(cmakeFlags.includes(flag));
});
test('each official archive and each model has an immutable verified pin', async () => {
  const pins = JSON.parse(await fs.readFile(new URL('../desktop-assets.json', import.meta.url), 'utf8'));
  assert.deepEqual(Object.keys(pins.piper.archives).sort(), ['linux-x64', 'win32-x64']);
  assert.equal(pins.piper.release, '2023.11.14-2');
  assert.equal(pins.piper.base_url, 'https://github.com/rhasspy/piper/releases/download/2023.11.14-2/');
  assert.deepEqual(pins.piper.archives['win32-x64'], {
    name: 'piper_windows_amd64.zip',
    url: 'https://api.github.com/repos/rhasspy/piper/releases/assets/135529797',
    accept: 'application/octet-stream',
    bytes: 22477236,
    sha256: 'f3c58906402b24f3a96d92145f58acba6d86c9b5db896d207f78dc80811efcea',
  });
  assert.deepEqual(pins.piper.archives['linux-x64'], {
    name: 'piper_linux_x86_64.tar.gz',
    url: 'https://api.github.com/repos/rhasspy/piper/releases/assets/135532855',
    accept: 'application/octet-stream',
    bytes: 26460462,
    sha256: 'a50cb45f355b7af1f6d758c1b360717877ba0a398cc8cbe6d2a7a3a26e225992',
  });
  for (const pin of Object.values(pins.piper.archives)) { assert.match(pin.sha256, /^[0-9a-f]{64}$/); assert.ok(pin.bytes > 0); }
  for (const pin of pins.files) { assert.match(pin.sha256, /^[0-9a-f]{64}$/); assert.match(pin.url, /\/resolve\/[0-9a-f]{40}\//); }
  assert.equal(pins.whisper.commit, '8a9ad7844d6e2a10cddf4b92de4089d7ac2b14a9');
  assert.deepEqual(Object.keys(pins.piper.macos_sources).sort(), ['espeak', 'fmt', 'phonemize', 'piper', 'sonic', 'spdlog']);
  for (const pin of Object.values(pins.piper.macos_sources)) {
    assert.match(pin.commit, /^[0-9a-f]{40}$/);
    assert.ok(pin.url.endsWith(`/tar.gz/${pin.commit}`));
    assert.match(pin.sha256, /^[0-9a-f]{64}$/); assert.ok(pin.bytes > 0);
  }
  for (const [platform, arch] of [['darwin-arm64', 'arm64'], ['darwin-x64', 'x86_64']]) {
    const pin = pins.piper.macos_onnxruntime[platform];
    assert.equal(pin.name, `onnxruntime-osx-${arch}-1.14.1.tgz`);
    assert.match(pin.sha256, /^[0-9a-f]{64}$/); assert.ok(pin.bytes > 0);
  }
});
