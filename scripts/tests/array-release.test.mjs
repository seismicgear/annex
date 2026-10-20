import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath } from 'node:url';
import { createHash, generateKeyPairSync } from 'node:crypto';
import { TARGETS, captureInventory, createEnvelopes, validateInventory } from '../array-release.mjs';
import { verifyRelease } from '../vendor/array/release-trust.mjs';

const now = Date.parse('2026-10-20T12:00:00.000Z');
const sourceCommit = 'a'.repeat(40);
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
async function fixture(t, platform = 'win32', arch = 'x64') {
  const root = await fs.mkdtemp(path.join(os.tmpdir(), 'annex-array-release-'));
  t.after(() => fs.rm(root, { recursive: true, force: true }));
  const bundle = path.join(root, 'bundle'), releaseDirectory = path.join(root, 'release');
  await fs.mkdir(bundle); await fs.mkdir(releaseDirectory);
  const executable = path.join(root, 'native-executable');
  await fs.writeFile(executable, 'component fixture only: installed executable');
  const names = { nsis: 'Annex_0.1.3_x64-setup.exe', msi: 'Annex_0.1.3_x64_en-US.msi',
    appimage: 'Annex_0.1.3_amd64.AppImage', deb: 'Annex_0.1.3_amd64.deb',
    dmg: `Annex_0.1.3_${arch === 'arm64' ? 'aarch64' : 'x64'}.dmg` };
  for (const target of TARGETS.filter(t => t.platform === platform && t.arch === arch)) {
    const bytes = `NOT AN INSTALLER: ${names[target.kind]}`;
    await fs.writeFile(path.join(bundle, names[target.kind]), bytes);
    await fs.writeFile(path.join(releaseDirectory, target.name), bytes);
  }
  const inventory = await captureInventory({ bundle, executable, platform, arch, releaseVersion: '0.1.3', sourceCommit });
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const keyId = 'annex-release-test';
  const policy = { keys: { [keyId]: { products: ['annex'], channels: ['stable'], publicKey: publicKey.export({ type: 'spki', format: 'pem' }) } },
    allowedArtifactHosts: ['github.com', 'store.kuykendall.industries'],
    productPublishers: { annex: { thumbprint: 'A'.repeat(40), teamId: 'A123456789' } } };
  return { root, bundle, executable, inventory, options: { inventory, releaseDirectory,
    baseUrl: 'https://github.com/seismicgear/annex/releases/download/v0.1.3', sequence: 3,
    publishedAt: '2026-10-20T12:00:00.000Z', expiresAt: '2027-10-20T12:00:00.000Z',
    keyId, privateKey: privateKey.export({ type: 'pkcs8', format: 'pem' }), policy, now } };
}
function consumer(f, envelope, overrides = {}) {
  return verifyRelease(envelope, { keys: f.options.policy.keys, hosts: f.options.policy.allowedArtifactHosts,
    product: 'annex', platform: f.inventory.platform, arch: f.inventory.arch, now, runtimeOSVersion: null, ...overrides });
}

test('uses the unmodified, digest-pinned Array and Store verifier', async () => {
  const directory = fileURLToPath(new URL('../vendor/array/', import.meta.url));
  const upstream = JSON.parse(await fs.readFile(path.join(directory, 'UPSTREAM.json'), 'utf8'));
  assert.equal(upstream.commit, '01985a14d48614b43f51f4847a499f5872b4bd70');
  for (const [name, hash] of Object.entries(upstream.sha256)) assert.equal(sha(await fs.readFile(path.join(directory, name))), hash);
});
for (const [platform, arch] of [['win32', 'x64'], ['linux', 'x64'], ['darwin', 'arm64'], ['darwin', 'x64']]) {
  test(`${platform}-${arch}: actual file hashes form a release the existing Array verifier accepts`, async t => {
    const f = await fixture(t, platform, arch);
    const outputs = await createEnvelopes(f.options);
    // Exactly one stream record: two formats at one sequence would equivocate.
    assert.equal(outputs.length, 1);
    const result = consumer(f, outputs[0].envelope);
    assert.equal(result.product, 'annex');
    assert.equal(result.version, '0.1.3');
    assert.equal(result.artifact.executable, f.inventory.artifacts[0].executable);
    assert.equal(result.artifact.executableSha256, f.inventory.artifacts[0].executableSha256);
    assert.equal(result.minOSVersion, platform === 'win32' ? '10.0.0' : platform === 'darwin' ? '20.0.0' : undefined);
  });
}
test('alternative MSI or DEB is selected explicitly, never published alongside the default in one stream', async t => {
  for (const [platform, kind] of [['win32', 'msi'], ['linux', 'deb']]) {
    const f = await fixture(t, platform);
    const result = await createEnvelopes({ ...f.options, kind });
    assert.equal(result.length, 1); assert.equal(consumer(f, result[0].envelope).artifact.kind, kind);
    await assert.rejects(createEnvelopes({ ...f.options, kind: 'pkg' }), /Choose one/);
  }
});
test('Array rejects tampering, other products, expired releases, rollback and sequence equivocation', async t => {
  const f = await fixture(t);
  const [{ envelope }] = await createEnvelopes(f.options);
  const changed = { ...envelope, payload: Buffer.from(Buffer.from(envelope.payload, 'base64').toString().replace('"annex"', '"array"')).toString('base64') };
  assert.throws(() => consumer(f, changed), { code: 'BAD_SIGNATURE' });
  assert.throws(() => consumer(f, envelope, { product: 'array' }), { code: 'UNTRUSTED_RELEASE' });
  assert.throws(() => consumer(f, envelope, { now: Date.parse('2028-01-01') }), { code: 'EXPIRED_RELEASE' });
  assert.throws(() => consumer(f, envelope, { highWater: 4 }), { code: 'REPLAYED_RELEASE' });
  assert.throws(() => consumer(f, envelope, { currentVersion: '0.1.4' }), { code: 'DOWNGRADE_BLOCKED' });
  assert.throws(() => consumer(f, envelope, { highWater: 3, highWaterDigest: '0'.repeat(64) }), { code: 'REPLAYED_RELEASE' });
});
test('refuses altered artifacts, source/version drift, missing packages and invented executable identities', async t => {
  const f = await fixture(t);
  assert.throws(() => validateInventory(f.inventory, { sourceCommit: 'b'.repeat(40) }), /source commit/);
  assert.throws(() => validateInventory(f.inventory, { releaseVersion: '0.1.4' }), /version/);
  const changed = structuredClone(f.inventory); changed.artifacts[0].executable = 'another.exe';
  assert.throws(() => validateInventory(changed), /identity/);
  await fs.writeFile(path.join(f.options.releaseDirectory, f.inventory.artifacts[0].name), 'corruption');
  await assert.rejects(createEnvelopes(f.options), /Release bytes differ/);
  await fs.unlink(path.join(f.bundle, 'Annex_0.1.3_x64_en-US.msi'));
  await assert.rejects(captureInventory({ bundle: f.bundle, executable: f.executable, platform: 'win32', arch: 'x64', releaseVersion: '0.1.3', sourceCommit }), /exactly one msi/);
});
test('fails closed without production authority and native publisher pins', async t => {
  const f = await fixture(t);
  for (const [override, message] of [
    [{ sequence: 0 }, /positive/], [{ keyId: 'dev-fixture' }, /production/],
    [{ baseUrl: 'http://github.com/releases' }, /not approved/],
    [{ baseUrl: 'https://evil.example/releases' }, /not approved/],
    [{ baseUrl: 'https://github.com/releases/dry-run-sha/' }, /Draft/],
    [{ policy: { ...f.options.policy, productPublishers: {} } }, /Authenticode/],
    [{ expiresAt: '2025-01-01T00:00:00Z' }, /expired|invalid publication/i],
  ]) await assert.rejects(createEnvelopes({ ...f.options, ...override }), message);
  const policy = structuredClone(f.options.policy);
  policy.keys[f.options.keyId].products.push('array');
  await assert.rejects(createEnvelopes({ ...f.options, policy }), /scoped only/);
  const key = generateKeyPairSync('ed25519').privateKey.export({ type: 'pkcs8', format: 'pem' });
  await assert.rejects(createEnvelopes({ ...f.options, privateKey: key }), /differs from approved/);
});
