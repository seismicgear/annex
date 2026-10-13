import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtemp, mkdir, readFile, writeFile, rm, readdir, copyFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createHash, generateKeyPairSync, randomBytes, sign } from 'node:crypto';
import { stageRelease, verifyUpdaterSignature } from '../stage-desktop-release.mjs';

// These are explicit release-packaging component fixtures, never installers.
// Independent Node crypto produces valid Minisign documents for known bytes.
function signingFixture() {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const keyID = randomBytes(8);
  const raw = publicKey.export({ type: 'spki', format: 'der' }).subarray(-32);
  const key = Buffer.from(`untrusted comment: disposable component test key\n${Buffer.concat([Buffer.from('Ed'), keyID, raw]).toString('base64')}\n`).toString('base64');
  return { key, signature(bytes, comment = 'timestamp:1\tfile:fixture') {
    const signature = sign(null, createHash('blake2b512').update(bytes).digest(), privateKey);
    const global = sign(null, Buffer.concat([signature, Buffer.from(comment)]), privateKey);
    return Buffer.from(`untrusted comment: disposable component signature\n${Buffer.concat([Buffer.from('ED'), keyID, signature]).toString('base64')}\ntrusted comment: ${comment}\n${global.toString('base64')}\n`).toString('base64');
  } };
}

async function fixture(t) {
  const root = await mkdtemp(join(tmpdir(), 'annex-release-test-'));
  t.after(() => rm(root, { recursive: true, force: true }));
  const artifacts = join(root, 'artifacts');
  const signer = signingFixture();
  const files = [
    ['annex-windows-x86_64', 'nsis/Annex_0.1.0_x64-setup.exe', true],
    ['annex-windows-x86_64', 'msi/Annex_0.1.0_x64_en-US.msi', true],
    ['annex-linux-x86_64', 'appimage/Annex_0.1.0_amd64.AppImage', true],
    ['annex-linux-x86_64', 'deb/Annex_0.1.0_amd64.deb', false],
    ['annex-macos-aarch64', 'dmg/Annex_0.1.0_aarch64.dmg', false],
    ['annex-macos-aarch64', 'macos/Annex.app.tar.gz', true],
    ['annex-macos-x86_64', 'dmg/Annex_0.1.0_x64.dmg', false],
    ['annex-macos-x86_64', 'macos/Annex.app.tar.gz', true],
  ];
  const written = [];
  for (const [group, name, signed] of files) {
    const file = join(artifacts, group, name);
    await mkdir(join(file, '..'), { recursive: true });
    const bytes = Buffer.from(`NOT AN INSTALLER: ${group}/${name}\n`);
    await writeFile(file, bytes);
    if (signed) await writeFile(`${file}.sig`, signer.signature(bytes));
    written.push(file);
  }
  return { root, artifacts, signer, written, options: { artifacts, output: join(root, 'release'), version: '0.1.0', tag: 'v0.1.0', publicKey: signer.key, sourceCommit: 'a'.repeat(40), publishedAt: '2026-10-13T00:00:00Z' } };
}

test('stages all native Tauri2 formats with independently verified signatures and final checksums', async t => {
  const f = await fixture(t);
  const result = await stageRelease(f.options);
  assert.equal(result.signed, true);
  assert.deepEqual(result.platforms, ['windows-x86_64', 'linux-x86_64', 'darwin-aarch64', 'darwin-x86_64']);
  const latest = JSON.parse(await readFile(join(f.options.output, 'latest.json')));
  assert.match(latest.platforms['windows-x86_64'].url, /annex-desktop-windows-x86_64\.exe$/);
  assert.match(latest.platforms['linux-x86_64'].url, /annex-desktop-linux-x86_64\.AppImage$/);
  assert.notEqual(latest.platforms['darwin-aarch64'].url, latest.platforms['darwin-x86_64'].url);
  assert.deepEqual(await readFile(join(f.options.output, 'annex-desktop-windows-x86_64.exe')), await readFile(f.written[0]));
  const sums = (await readFile(join(f.options.output, 'SHA256SUMS'), 'utf8')).trim().split('\n');
  assert.ok(sums.some(line => line.endsWith('  latest.json')));
  assert.ok(sums.some(line => line.endsWith('  downloads.json')));
  assert.equal(sums.length, (await readdir(f.options.output)).length - 1);
  for (const line of sums) {
    const [hash, name] = line.split('  ');
    assert.equal(createHash('sha256').update(await readFile(join(f.options.output, name))).digest('hex'), hash);
  }
});

test('refuses changed installer bytes even when a signature file exists', async t => {
  const f = await fixture(t);
  await writeFile(f.written[0], 'changed');
  await assert.rejects(stageRelease(f.options), /invalid updater signature/);
});

test('refuses a signature from another production key', async t => {
  const f = await fixture(t);
  await assert.rejects(stageRelease({ ...f.options, publicKey: signingFixture().key }), /key does not match/);
});

test('authenticates the signature trusted comment', async t => {
  const f = await fixture(t);
  const original = await readFile(`${f.written[0]}.sig`, 'utf8');
  const changed = Buffer.from(Buffer.from(original, 'base64').toString().replace('file:fixture', 'file:changed')).toString('base64');
  await assert.rejects(verifyUpdaterSignature(f.written[0], changed, f.signer.key), /invalid updater signature/);
});

test('normalizes a valid plain signature to the updater base64 wire format', async t => {
  const f = await fixture(t);
  const original = await readFile(`${f.written[0]}.sig`, 'utf8');
  await writeFile(`${f.written[0]}.sig`, Buffer.from(original, 'base64'));
  await stageRelease(f.options);
  const latest = JSON.parse(await readFile(join(f.options.output, 'latest.json')));
  assert.equal(latest.platforms['windows-x86_64'].signature, original);
});

test('requires every requested platform rather than publishing a partial matrix', async t => {
  const f = await fixture(t);
  await rm(join(f.artifacts, 'annex-macos-aarch64'), { recursive: true });
  await assert.rejects(stageRelease(f.options), /ENOENT/);
});

test('refuses duplicate artifact candidates', async t => {
  const f = await fixture(t);
  await copyFile(f.written[0], join(f.written[0], '..', 'another.exe'));
  await assert.rejects(stageRelease(f.options), /expected one artifact, found 2/);
});

test('refuses missing updater signatures and obsolete v1-only output', async t => {
  const f = await fixture(t);
  await rm(`${f.written[0]}.sig`);
  await writeFile(join(f.written[0], '..', 'old.nsis.zip.sig'), 'not the native installer signature');
  await assert.rejects(stageRelease(f.options), /ENOENT/);
});

test('refuses mixing release output from another attempt', async t => {
  const f = await fixture(t);
  await mkdir(f.options.output);
  await writeFile(join(f.options.output, 'stale.exe'), 'old');
  await assert.rejects(stageRelease(f.options), /must be empty/);
});

test('unsigned mode is restricted to a named dry-run draft', async t => {
  const f = await fixture(t);
  await assert.rejects(stageRelease({ ...f.options, unsignedDryRun: true }), /explicit dry-run draft/);
  const result = await stageRelease({ ...f.options, tag: `dry-run-${'a'.repeat(40)}`, publicKey: undefined, unsignedDryRun: true });
  assert.equal(result.signed, false);
  assert.equal(result.assets.includes('latest.json'), false);
  assert.equal(JSON.parse(await readFile(join(f.options.output, 'downloads.json'))).unsignedDryRun, true);
});

test('refuses missing signing key before producing public metadata', async t => {
  const f = await fixture(t);
  await assert.rejects(stageRelease({ ...f.options, publicKey: undefined }), /production updater public key is required/);
});

test('refuses version/tag mismatch and unknown source', async t => {
  const f = await fixture(t);
  await assert.rejects(stageRelease({ ...f.options, tag: 'v0.2.0' }), /tag and version disagree/);
  await assert.rejects(stageRelease({ ...f.options, sourceCommit: 'main' }), /exact source commit/);
});
