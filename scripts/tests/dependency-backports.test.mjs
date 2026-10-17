import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { regularFile, verifyBackports } from '../verify-dependency-backports.mjs';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '../..');
async function fixture(t) {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'annex-backport-test-'));
  t.after(async () => {
    assert.equal(path.dirname(root), path.resolve(os.tmpdir()));
    assert.ok(path.basename(root).startsWith('annex-backport-test-'));
    await fs.rm(root, { recursive: true, force: true });
  });
  await fs.mkdir(path.join(root, 'scripts'));
  await fs.cp(path.join(repo, 'vendor'), path.join(root, 'vendor'), { recursive: true });
  await fs.cp(path.join(repo, 'scripts/security-regressions'), path.join(root, 'scripts/security-regressions'), { recursive: true, filter: source => !source.split(path.sep).includes('target') });
  await fs.cp(path.join(repo, 'scripts/updater-archive-regressions'), path.join(root, 'scripts/updater-archive-regressions'), { recursive: true, filter: source => !source.split(path.sep).includes('target') });
  await fs.cp(path.join(repo, 'scripts/single-instance-regressions'), path.join(root, 'scripts/single-instance-regressions'), { recursive: true, filter: source => !source.split(path.sep).includes('target') });
  await fs.copyFile(path.join(repo, 'scripts/dependency-backports.json'), path.join(root, 'scripts/dependency-backports.json'));
  const packages = ['glib', 'ark-relations'].map((name, i) => ({ name, version: i ? '0.5.1' : '0.18.5', id: `path:${name}`, source: null, manifest_path: path.join(root, 'vendor', name, 'Cargo.toml') }));
  packages.push({ name: 'tracing-subscriber', version: '0.3.23', id: 'registry:tracing-subscriber', source: 'registry' });
  packages.push({ name: 'tauri-plugin-updater', version: '2.14.0', id: 'path:tauri-plugin-updater', source: null, manifest_path: path.join(root, 'vendor/tauri-plugin-updater/Cargo.toml') });
  packages.push({ name: 'zip', version: '9.0.3', id: 'registry:zip', source: 'registry' });
  packages.push({ name: 'tauri-plugin-single-instance', version: '2.5.4', id: 'path:tauri-plugin-single-instance', source: null, manifest_path: path.join(root, 'vendor/tauri-plugin-single-instance/Cargo.toml') });
  packages.push({ name: 'zbus', version: '6.0.0', id: 'registry:zbus', source: 'registry' });
  const metadata = { packages, resolve: { nodes: [{ id: 'path:ark-relations', dependencies: ['registry:tracing-subscriber'] }, { id: 'path:tauri-plugin-updater', dependencies: ['registry:zip'] }, { id: 'path:tauri-plugin-single-instance', dependencies: ['registry:zbus'] }] } };
  return { root, metadata };
}

test('reconstructs every exact vendored source byte and verifies resolved Cargo paths', async t => {
  const { root, metadata } = await fixture(t);
  assert.deepEqual(await verifyBackports(root, metadata), { crates: ['glib@0.18.5', 'ark-relations@0.5.1', 'tauri-plugin-updater@2.14.0', 'tauri-plugin-single-instance@2.5.4'], resolved: true });
});

test('single-instance cannot retain a retired D-Bus dependency or a second old bus', async t => {
  const { root, metadata } = await fixture(t);
  for (const name of ['zbus_names', 'zvariant', 'zvariant_derive', 'zbus']) {
    metadata.packages.push({ name, version: '5.14.0', id: `registry:old-${name}`, source: 'registry' });
    await assert.rejects(verifyBackports(root, metadata), /maintained zbus/);
    metadata.packages.pop();
  }
  metadata.packages.find(pkg => pkg.name === 'zbus').version = '5.19.0';
  await assert.rejects(verifyBackports(root, metadata), /maintained zbus/);
});
test('removing the GLib mutability fix fails before any audit exception can apply', async t => {
  const { root, metadata } = await fixture(t);
  const filename = path.join(root, 'vendor/glib/src/variant_iter.rs');
  await fs.writeFile(filename, (await fs.readFile(filename, 'utf8')).replace('&mut p,', '&p,'));
  await assert.rejects(verifyBackports(root, metadata), /exact reviewed backport/);
});
test('an archive checksum mismatch or extra vendored file is refused', async t => {
  const { root, metadata } = await fixture(t);
  const archive = path.join(root, 'vendor/upstream/glib-0.18.5.crate');
  const bytes = await fs.readFile(archive);
  await fs.appendFile(archive, 'corrupt');
  await assert.rejects(verifyBackports(root, metadata), /archive checksum mismatch/);
  await fs.writeFile(archive, bytes);
  await fs.writeFile(path.join(root, 'vendor/glib/unreviewed.rs'), 'unreviewed');
  await assert.rejects(verifyBackports(root, metadata), /file inventory changed/);
});
test('a removed Cargo patch cannot waive the original registry GLib package', async t => {
  const { root, metadata } = await fixture(t);
  metadata.packages[0].source = 'registry+https://github.com/rust-lang/crates.io-index';
  await assert.rejects(verifyBackports(root, metadata), /did not resolve the verified vendored crate/);
});
test('another vulnerable GLib version cannot inherit the reviewed backport disposition', async t => {
  const { root, metadata } = await fixture(t);
  metadata.packages.push({ name: 'glib', version: '0.19.9', id: 'registry:glib-old', source: 'registry' });
  await assert.rejects(verifyBackports(root, metadata), /another GLib version affected/);
});
test('Ark must resolve a fixed subscriber and preserve the reviewed callback migration', async t => {
  const { root, metadata } = await fixture(t);
  metadata.packages[2].version = '0.2.25';
  await assert.rejects(verifyBackports(root, metadata), /repaired tracing subscriber/);
  metadata.packages[2].version = '0.3.23';
  const filename = path.join(root, 'vendor/ark-relations/src/r1cs/trace.rs');
  await fs.writeFile(filename, (await fs.readFile(filename, 'utf8')).replace('fn on_new_span(', 'fn new_span('));
  await assert.rejects(verifyBackports(root, metadata), /exact reviewed backport/);
});
test('removing a runtime regression cannot leave the source gate green', async t => {
  const { root, metadata } = await fixture(t);
  await fs.writeFile(path.join(root, 'scripts/security-regressions/src/lib.rs'), '// omitted tests\n');
  await assert.rejects(verifyBackports(root, metadata), /regression gate changed/);
});

test('the updater cannot resolve a withdrawn ZIP reader or retain a second copy', async t => {
  const { root, metadata } = await fixture(t);
  const archive = metadata.packages.find(pkg => pkg.name === 'zip');
  archive.version = '4.6.1';
  await assert.rejects(verifyBackports(root, metadata), /supported ZIP reader/);
  archive.version = '9.0.3';
  metadata.packages.push({ ...archive, version: '4.6.1', id: 'registry:zip-old' });
  await assert.rejects(verifyBackports(root, metadata), /supported ZIP reader/);
});

test('reads the verified descriptor when its pathname is replaced after inspection', async t => {
  const { root } = await fixture(t);
  const filename = path.join(root, 'opened-file');
  await fs.writeFile(filename, 'verified original bytes');
  const lstat = fs.lstat.bind(fs);
  t.mock.method(fs, 'lstat', async (target, options) => {
    const inspected = await lstat(target, options);
    if (target === filename) {
      await fs.rename(filename, `${filename}.original`);
      await fs.writeFile(filename, 'unreviewed replacement bytes');
    }
    return inspected;
  });
  assert.equal((await regularFile(filename)).toString(), 'verified original bytes');
});

test('refuses a pathname changed between opening and identity inspection', async t => {
  const { root } = await fixture(t);
  const filename = path.join(root, 'opened-file');
  await fs.writeFile(filename, 'original bytes');
  const lstat = fs.lstat.bind(fs);
  t.mock.method(fs, 'lstat', async (target, options) => {
    if (target === filename) {
      await fs.rename(filename, `${filename}.original`);
      await fs.writeFile(filename, 'replacement bytes');
    }
    return lstat(target, options);
  });
  await assert.rejects(regularFile(filename), /same regular file/);
});

test('refuses actual symbolic filesystem entries', async t => {
  const { root } = await fixture(t);
  const link = path.join(root, 'link');
  if (process.platform === 'win32') {
    // Junction creation requires no Administrator or Developer Mode setting.
    // File symlink refusal is also exercised on the Linux/macOS CI runners.
    await fs.symlink(path.join(root, 'vendor'), link, 'junction');
  } else {
    await fs.symlink(path.join(root, 'vendor/glib/Cargo.toml'), link);
  }
  await assert.rejects(regularFile(link), /ELOOP|EISDIR|EPERM|same regular file/);
});
