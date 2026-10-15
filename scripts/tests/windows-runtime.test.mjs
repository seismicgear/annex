import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { createHash } from 'node:crypto';
import { inspectDirectory, inspectRuntimeProvenance, isInboxImport, peImports } from '../windows-runtime.mjs';

// Minimal PE64 fixtures exercise the parser only. Native acceptance uses actual
// packaged executable files and a clean Windows installation.
function peFixture(imports = [], delayed = []) {
  const bytes = Buffer.alloc(0x1000);
  bytes.writeUInt16LE(0x5a4d, 0);
  bytes.writeUInt32LE(0x80, 0x3c);
  bytes.writeUInt32LE(0x4550, 0x80);
  bytes.writeUInt16LE(0x8664, 0x84);
  bytes.writeUInt16LE(1, 0x86);
  bytes.writeUInt16LE(240, 0x94);
  const optional = 0x98;
  bytes.writeUInt16LE(0x20b, optional);
  bytes.writeUInt32LE(16, optional + 108);
  const section = optional + 240;
  bytes.writeUInt32LE(0x1000, section + 12);
  bytes.writeUInt32LE(0xe00, section + 16);
  bytes.writeUInt32LE(0x200, section + 20);
  let stringOffset = 0x500;
  for (const [entries, directory, offset, stride, nameOffset] of [
    [imports, 1, 0x200, 20, 12], [delayed, 13, 0x380, 32, 4],
  ]) {
    if (!entries.length) continue;
    bytes.writeUInt32LE(offset + 0xe00, optional + 112 + directory * 8);
    bytes.writeUInt32LE((entries.length + 1) * stride, optional + 116 + directory * 8);
    for (const [index, name] of entries.entries()) {
      const descriptor = offset + index * stride;
      if (directory === 13) bytes.writeUInt32LE(1, descriptor);
      bytes.writeUInt32LE(stringOffset + 0xe00, descriptor + nameOffset);
      stringOffset += bytes.write(name + '\0', stringOffset, 'ascii');
    }
  }
  return bytes;
}

test('reads ordinary and delay-loaded PE imports without missing CRT dependencies', () => {
  assert.deepEqual(peImports(peFixture(['KERNEL32.dll', 'MSVCP140.dll'], ['VCRUNTIME140_1.dll', 'kernel32.dll'])),
    ['kernel32.dll', 'msvcp140.dll', 'vcruntime140_1.dll']);
  assert.deepEqual(peImports(peFixture()), []);
});

test('rejects malformed, truncated and non-x64 PE files', () => {
  assert.throws(() => peImports(Buffer.alloc(4)), /Not a PE/);
  assert.throws(() => peImports(peFixture().subarray(0, 0x190)), /Truncated/);
  const x86 = peFixture();
  x86.writeUInt16LE(0x14c, 0x84);
  assert.throws(() => peImports(x86), /x64/);
  const outside = peFixture(['MSVCP140.dll']);
  outside.writeUInt32LE(0xffff0000, 0x200 + 12);
  assert.throws(() => peImports(outside), /outside raw sections/);
  const unterminated = peFixture(['MSVCP140.dll']);
  unterminated.writeUInt32LE(20, 0x98 + 116 + 8);
  assert.throws(() => peImports(unterminated), /Unterminated/);
  const delay = peFixture([], ['MSVCP140.dll']);
  delay.writeUInt32LE(0, 0x380);
  assert.throws(() => peImports(delay), /non-RVA delay/);
  assert.throws(() => peImports(peFixture(['../MSVCP140.dll'])), /Unsafe PE import/);
});

test('only Windows inbox libraries are exempt from local closure', () => {
  for (const name of ['KERNEL32.DLL', 'ole32.dll', 'api-ms-win-crt-heap-l1-1-0.dll', 'ext-ms-win-ntuser-window-l1-1-0.dll']) {
    assert.equal(isInboxImport(name), true, name);
  }
  for (const name of ['msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll', 'onnxruntime.dll', 'private-engine.dll']) {
    assert.equal(isInboxImport(name), false, name);
  }
});

test('requires app-local and transitive runtime libraries even on a developer machine', async t => {
  const tempRoot = path.resolve(os.tmpdir());
  const directory = await fs.mkdtemp(path.join(tempRoot, 'annex-runtime-test-'));
  t.after(async () => {
    assert.equal(path.dirname(path.resolve(directory)), tempRoot);
    assert.match(path.basename(directory), /^annex-runtime-test-/);
    await fs.rm(directory, { recursive: true, force: true });
  });
  await fs.writeFile(path.join(directory, 'annex-desktop.exe'), peFixture(['KERNEL32.dll', 'MSVCP140.dll']));
  await assert.rejects(inspectDirectory(directory, 'annex-desktop.exe'), /annex-desktop.exe has unbundled imports: msvcp140.dll/);
  await fs.writeFile(path.join(directory, 'MSVCP140.dll'), peFixture([], ['VCRUNTIME140.dll']));
  await assert.rejects(inspectDirectory(directory, 'annex-desktop.exe'), /MSVCP140.dll has unbundled imports: vcruntime140.dll/);
  await fs.writeFile(path.join(directory, 'vcruntime140.dll'), peFixture(['api-ms-win-crt-heap-l1-1-0.dll']));
  const inventory = await inspectDirectory(directory, 'annex-desktop.exe');
  assert.equal(inventory.length, 3);
  assert.match(inventory[0].sha256, /^[a-f0-9]{64}$/);
  // NSIS's independent x86 uninstaller is not an application runtime dependency.
  await fs.writeFile(path.join(directory, 'uninstall.exe'), Buffer.alloc(8));
  assert.equal((await inspectDirectory(directory, 'annex-desktop.exe')).length, 3);
});

test('Windows resources preserve every product asset and place runtime libraries beside all executables', async () => {
  const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
  const base = JSON.parse(await fs.readFile(path.join(root, 'crates/annex-desktop/tauri.conf.json'), 'utf8'));
  const windows = JSON.parse(await fs.readFile(path.join(root, 'crates/annex-desktop/tauri.windows.conf.json'), 'utf8'));
  const resources = windows.bundle.resources;
  const normalized = new Map(Object.entries(resources).map(([source, target]) => [source.replace(/\/$/, ''), target.replace(/\/$/, '')]));
  for (const source of base.bundle.resources) {
    assert.equal(normalized.get(source), source.replaceAll('../', '_up_/'), source);
  }
  assert.equal(resources['../../assets/windows-runtime/app/'], '');
  assert.equal(resources['../../assets/windows-runtime/provenance.json'], 'windows-runtime-provenance.json');
  assert.equal(resources['../../assets/windows-runtime/piper/'], '_up_/_up_/assets/piper/');
  assert.equal(resources['../../assets/windows-runtime/whisper/'], '_up_/_up_/assets/whisper/');
  assert.equal(base.bundle.targets, 'all');
  assert.equal(windows.bundle.targets, undefined, 'both NSIS and MSI inherit the same payload');
});

test('packaged provenance binds every component runtime to its recorded bytes', async t => {
  const tempRoot = path.resolve(os.tmpdir());
  const directory = await fs.mkdtemp(path.join(tempRoot, 'annex-runtime-test-'));
  t.after(async () => {
    assert.equal(path.dirname(path.resolve(directory)), tempRoot);
    assert.match(path.basename(directory), /^annex-runtime-test-/);
    await fs.rm(directory, { recursive: true, force: true });
  });
  const bytes = peFixture(['KERNEL32.dll']);
  const receipt = { version: 1, architecture: 'x86_64', source: 'Visual Studio 2022 Microsoft.VC143.CRT', files: [] };
  for (const [component, componentRoot] of [
    ['app', directory], ['piper', path.join(directory, '_up_', '_up_', 'assets', 'piper')],
    ['whisper', path.join(directory, '_up_', '_up_', 'assets', 'whisper')],
  ]) {
    await fs.mkdir(componentRoot, { recursive: true });
    for (const name of ['msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll']) {
      await fs.writeFile(path.join(componentRoot, name), bytes);
      receipt.files.push({ path: `${component}/${name}`, bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') });
    }
  }
  const receiptPath = path.join(directory, 'windows-runtime-provenance.json');
  await fs.writeFile(receiptPath, JSON.stringify(receipt));
  assert.equal((await inspectRuntimeProvenance(directory)).files.length, 9);
  const changed = Buffer.from(bytes);
  changed[bytes.length - 1] = 1;
  await fs.writeFile(path.join(directory, '_up_', '_up_', 'assets', 'piper', 'msvcp140.dll'), changed);
  await assert.rejects(inspectRuntimeProvenance(directory), /differs from provenance: piper\/msvcp140.dll/);
  receipt.files[0].path = 'app/../../elsewhere';
  await fs.writeFile(receiptPath, JSON.stringify(receipt));
  await assert.rejects(inspectRuntimeProvenance(directory), /Invalid or repeated/);
});
