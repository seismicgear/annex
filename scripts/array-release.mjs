#!/usr/bin/env node
// Annex's side of Array's existing signed-release contract. Never invent an
// installed hash, publisher, entitlement, production key, or download location.
import fs from 'node:fs/promises';
import { createReadStream } from 'node:fs';
import { createHash, createPrivateKey, createPublicKey, sign } from 'node:crypto';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { verifyRelease } from './vendor/array/release-trust.mjs';

export const TARGETS = Object.freeze([
  { platform: 'win32', arch: 'x64', kind: 'nsis', pattern: /_x64-setup\.exe$/, name: 'annex-desktop-windows-x86_64.exe', executable: 'annex-desktop.exe' },
  { platform: 'win32', arch: 'x64', kind: 'msi', pattern: /_x64_en-US\.msi$/, name: 'annex-desktop-windows-x86_64.msi', executable: 'annex-desktop.exe' },
  { platform: 'linux', arch: 'x64', kind: 'appimage', pattern: /_amd64\.AppImage$/, name: 'annex-desktop-linux-x86_64.AppImage', executable: 'annex-desktop-linux-x86_64.AppImage' },
  { platform: 'linux', arch: 'x64', kind: 'deb', pattern: /_amd64\.deb$/, name: 'annex-desktop-linux-x86_64.deb', executable: '/usr/bin/annex-desktop' },
  { platform: 'darwin', arch: 'arm64', kind: 'dmg', pattern: /_aarch64\.dmg$/, name: 'annex-desktop-macos-aarch64.dmg', executable: 'Annex.app/Contents/MacOS/annex-desktop' },
  { platform: 'darwin', arch: 'x64', kind: 'dmg', pattern: /_x64\.dmg$/, name: 'annex-desktop-macos-x86_64.dmg', executable: 'Annex.app/Contents/MacOS/annex-desktop' },
]);

function check(value, message) { if (!value) throw new Error(message); }
function version(value) { check(typeof value === 'string' && /^\d{1,6}\.\d{1,6}\.\d{1,6}$/.test(value), 'Array requires a strict x.y.z version'); }
function commit(value) { check(typeof value === 'string' && /^[a-f0-9]{40}$/.test(value), 'An exact source commit is required'); }
export async function hashFile(file) {
  const stat = await fs.lstat(file);
  check(stat.isFile() && !stat.isSymbolicLink() && stat.size > 0 && stat.size <= 16 * 1024 ** 3, 'Expected a nonempty regular release file');
  const hash = createHash('sha256');
  for await (const bytes of createReadStream(file)) hash.update(bytes);
  return { size: stat.size, sha256: hash.digest('hex') };
}
async function filesWithin(directory) {
  const files = [];
  for (const entry of await fs.readdir(directory, { withFileTypes: true })) {
    // Bundled macOS frameworks may contain legitimate symlinks. They are not
    // candidates for installers, and must never be traversed by this scanner.
    if (entry.isSymbolicLink()) continue;
    const file = path.join(directory, entry.name);
    if (entry.isDirectory()) files.push(...await filesWithin(file));
    else if (entry.isFile()) files.push(file);
  }
  return files;
}
export function validateInventory(inventory, { sourceCommit, releaseVersion } = {}) {
  check(inventory?.schemaVersion === 1 && inventory.product === 'annex' && inventory.nativeId === 'com.annex.desktop', 'Invalid Annex installation inventory');
  commit(inventory.sourceCommit); version(inventory.version);
  if (sourceCommit) check(inventory.sourceCommit === sourceCommit, 'Installation inventory source commit differs from release');
  if (releaseVersion) check(inventory.version === releaseVersion, 'Installation inventory version differs from release');
  const targets = TARGETS.filter(t => t.platform === inventory.platform && t.arch === inventory.arch);
  check(targets.length > 0 && Array.isArray(inventory.artifacts) && inventory.artifacts.length === targets.length, 'Incomplete native installation inventory');
  for (const target of targets) {
    const entries = inventory.artifacts.filter(a => a.kind === target.kind);
    check(entries.length === 1, 'Duplicate or missing native installer');
    const entry = entries[0];
    check(entry.name === target.name && entry.executable === target.executable, 'Native installer identity differs from Array adapter');
    check(Number.isSafeInteger(entry.size) && entry.size > 0 && entry.size <= 16 * 1024 ** 3, 'Invalid native installer size');
    check(/^[a-f0-9]{64}$/.test(entry.sha256) && /^[a-f0-9]{64}$/.test(entry.executableSha256), 'Invalid native installer digest');
    if (target.kind === 'appimage') check(entry.sha256 === entry.executableSha256, 'AppImage must be its own installed executable');
  }
  return inventory;
}
export async function captureInventory({ bundle, executable, platform, arch, releaseVersion, sourceCommit }) {
  version(releaseVersion); commit(sourceCommit);
  const targets = TARGETS.filter(t => t.platform === platform && t.arch === arch);
  check(targets.length, 'Unsupported Annex platform or architecture');
  const installed = await hashFile(executable);
  const files = await filesWithin(bundle);
  const artifacts = [];
  for (const target of targets) {
    const matches = files.filter(file => target.pattern.test(path.basename(file)) && path.basename(file).startsWith(`Annex_${releaseVersion}_`));
    check(matches.length === 1, `Expected exactly one ${target.kind} installer for Annex ${releaseVersion}`);
    const digest = await hashFile(matches[0]);
    artifacts.push({ kind: target.kind, name: target.name, ...digest, executable: target.executable,
      executableSha256: target.kind === 'appimage' ? digest.sha256 : installed.sha256 });
  }
  return validateInventory({ schemaVersion: 1, product: 'annex', nativeId: 'com.annex.desktop',
    version: releaseVersion, sourceCommit, platform, arch, artifacts });
}

// Production policy is supplied by the owner and used unchanged by Array and
// Store. The output is signed only after installer bytes and policy agree.
export async function createEnvelopes({ inventory, releaseDirectory, baseUrl, sequence, publishedAt, expiresAt, policy, keyId, privateKey, kind, now = Date.now() }) {
  validateInventory(inventory);
  check(Number.isSafeInteger(sequence) && sequence > 0, 'A positive release sequence is required');
  check(typeof keyId === 'string' && /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/.test(keyId) && !/^dev[-_:]/i.test(keyId), 'An approved production release key ID is required');
  const approved = policy?.keys?.[keyId];
  check(approved?.products?.length === 1 && approved.products[0] === 'annex' &&
    approved.channels?.length === 1 && approved.channels[0] === 'stable', 'Release key must be scoped only to Annex stable');
  check(Array.isArray(policy.allowedArtifactHosts) && policy.allowedArtifactHosts.length, 'Approved artifact hosts are required');
  const base = new URL(baseUrl);
  check(base.protocol === 'https:' && !base.username && !base.password && !base.hash && !base.search &&
    (!base.port || base.port === '443') && policy.allowedArtifactHosts.includes(base.hostname), 'Release base URL is not approved');
  check(!base.pathname.split('/').some(segment => /^dry-run-/i.test(segment)), 'Draft URLs cannot be signed as stable Array releases');
  // Native publisher verification remains mandatory in Array's existing adapter.
  const publisher = policy.productPublishers?.annex;
  if (inventory.platform === 'win32') check(/^[a-fA-F0-9]{40}$/.test(publisher?.thumbprint || ''), 'Annex Authenticode publisher pin is required');
  if (inventory.platform === 'darwin') check(/^[A-Z0-9]{10}$/.test(publisher?.teamId || ''), 'Annex Apple Team ID pin is required');
  const key = createPrivateKey(privateKey);
  check(key.asymmetricKeyType === 'ed25519', 'Array releases require an Ed25519 private key');
  const publicKey = createPublicKey(key).export({ type: 'spki', format: 'der' });
  check(publicKey.equals(createPublicKey(approved.publicKey).export({ type: 'spki', format: 'der' })), 'Signing key differs from approved Array policy');
  // Store's release stream is keyed by product/platform/arch/channel, not kind.
  // Signing NSIS and MSI at the same sequence would be equivocation. Publish
  // one selected installer (NSIS/AppImage/DMG by default); keep alternatives as
  // ordinary direct downloads.
  const selected = kind || TARGETS.find(t => t.platform === inventory.platform && t.arch === inventory.arch).kind;
  const entries = inventory.artifacts.filter(entry => entry.kind === selected);
  check(entries.length === 1, 'Choose one supported installer kind for this release stream');
  const envelopes = [];
  for (const entry of entries) {
    const actual = await hashFile(path.join(releaseDirectory, entry.name));
    check(actual.sha256 === entry.sha256 && actual.size === entry.size, `Release bytes differ from installation inventory: ${entry.name}`);
    const payload = { schemaVersion: 1, product: 'annex', version: inventory.version, sequence, channel: 'stable',
      platform: inventory.platform, arch: inventory.arch, minArrayVersion: '0.1.0', publishedAt, expiresAt,
      ...(inventory.platform === 'win32' ? { minOSVersion: '10.0.0' } : inventory.platform === 'darwin' ? { minOSVersion: '20.0.0' } : {}),
      artifact: { kind: entry.kind, url: new URL(entry.name, base.href.replace(/\/?$/, '/')).href,
        size: entry.size, sha256: entry.sha256, executable: entry.executable, executableSha256: entry.executableSha256 } };
    const bytes = Buffer.from(JSON.stringify(payload));
    const envelope = { schemaVersion: 1, keyId, payload: bytes.toString('base64'), signature: sign(null, bytes, key).toString('base64') };
    verifyRelease(envelope, { keys: policy.keys, hosts: policy.allowedArtifactHosts, product: 'annex',
      platform: inventory.platform, arch: inventory.arch, channel: 'stable', now, runtimeOSVersion: null });
    envelopes.push({ name: `${entry.name}.envelope.json`, envelope });
  }
  return envelopes;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    const { values: args } = parseArgs({ options: Object.fromEntries([
      'mode', 'bundle', 'executable', 'platform', 'arch', 'version', 'commit', 'out',
      'inventory', 'release-directory', 'base-url', 'sequence', 'published-at', 'expires-at', 'policy', 'kind',
    ].map(name => [name, { type: 'string' }])) });
    if (args.mode === 'capture') {
      const inventory = await captureInventory({ bundle: args.bundle, executable: args.executable,
        platform: args.platform, arch: args.arch, releaseVersion: args.version, sourceCommit: args.commit });
      await fs.writeFile(args.out, JSON.stringify(inventory, null, 2) + '\n', { flag: 'wx' });
      console.log(`Recorded ${inventory.artifacts.length} actual Annex installers for ${inventory.platform}-${inventory.arch}.`);
    } else if (args.mode === 'sign') {
      const outputs = await createEnvelopes({ inventory: JSON.parse(await fs.readFile(args.inventory, 'utf8')),
        releaseDirectory: args['release-directory'], baseUrl: args['base-url'], sequence: Number(args.sequence),
        publishedAt: args['published-at'], expiresAt: args['expires-at'], policy: JSON.parse(await fs.readFile(args.policy, 'utf8')),
        keyId: process.env.ANNEX_ARRAY_RELEASE_KEY_ID, privateKey: process.env.ANNEX_ARRAY_RELEASE_PRIVATE_KEY, kind: args.kind });
      await fs.mkdir(args.out, { recursive: true });
      check((await fs.readdir(args.out)).length === 0, 'Envelope output must be empty');
      for (const { name, envelope } of outputs) await fs.writeFile(path.join(args.out, name), JSON.stringify(envelope, null, 2) + '\n', { flag: 'wx' });
      console.log(`Signed and independently verified ${outputs.length} Annex release envelopes.`);
    } else throw new Error('Use --mode capture or --mode sign; see docs/architecture/array-distribution.md');
  } catch (error) { console.error(error.message); process.exitCode = 1; }
}
