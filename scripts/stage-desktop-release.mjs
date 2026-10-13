#!/usr/bin/env node
// Build the public release from actual Tauri 2 outputs. No installer is synthesized.
import { createReadStream } from 'node:fs';
import { copyFile, lstat, mkdir, readdir, readFile, writeFile } from 'node:fs/promises';
import { createHash, createPublicKey, verify } from 'node:crypto';
import { basename, dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const platforms = [
  { key: 'windows-x86_64', group: 'annex-windows-x86_64', installer: /\.exe$/, name: 'annex-desktop-windows-x86_64.exe', updater: /\.exe$/, updateName: 'annex-desktop-windows-x86_64.exe', optional: [{ pattern: /\.msi$/, name: 'annex-desktop-windows-x86_64.msi' }] },
  { key: 'linux-x86_64', group: 'annex-linux-x86_64', installer: /\.AppImage$/, name: 'annex-desktop-linux-x86_64.AppImage', updater: /\.AppImage$/, updateName: 'annex-desktop-linux-x86_64.AppImage', optional: [{ pattern: /\.deb$/, name: 'annex-desktop-linux-x86_64.deb' }] },
  { key: 'darwin-aarch64', group: 'annex-macos-aarch64', installer: /\.dmg$/, name: 'annex-desktop-macos-aarch64.dmg', updater: /\.app\.tar\.gz$/, updateName: 'annex-desktop-macos-aarch64.app.tar.gz', optional: [] },
  { key: 'darwin-x86_64', group: 'annex-macos-x86_64', installer: /\.dmg$/, name: 'annex-desktop-macos-x86_64.dmg', updater: /\.app\.tar\.gz$/, updateName: 'annex-desktop-macos-x86_64.app.tar.gz', optional: [] },
];

function checkedBase64(text, label) {
  const value = text.trim();
  if (!/^[A-Za-z0-9+/]+={0,2}$/.test(value) || value.length % 4 !== 0) throw new Error(`${label}: invalid base64`);
  const bytes = Buffer.from(value, 'base64');
  if (bytes.toString('base64') !== value) throw new Error(`${label}: noncanonical base64`);
  return bytes;
}

function minisignText(value, label) {
  if (typeof value !== 'string' || value.length > 16384) throw new Error(`${label}: missing or oversized`);
  return value.startsWith('untrusted comment: ') ? value : checkedBase64(value, label).toString('utf8');
}

export async function verifyUpdaterSignature(file, signatureValue, publicKeyValue) {
  // Tauri wraps the ordinary Minisign documents in base64. Verify both the
  // streaming BLAKE2b/Ed25519 file signature and its authenticated comment.
  const keyLines = minisignText(publicKeyValue, 'updater public key').trim().split(/\r?\n/);
  if (keyLines.length !== 2 || !keyLines[0].startsWith('untrusted comment: ')) throw new Error('invalid updater public key document');
  const key = checkedBase64(keyLines[1], 'updater public key');
  if (key.length !== 42 || !['Ed', 'ED'].includes(key.subarray(0, 2).toString())) throw new Error('unsupported updater public key');
  const lines = minisignText(signatureValue, 'updater signature').trim().split(/\r?\n/);
  if (lines.length !== 4 || !lines[0].startsWith('untrusted comment: ') || !lines[2].startsWith('trusted comment: ')) throw new Error('invalid updater signature document');
  const signed = checkedBase64(lines[1], 'updater signature');
  const global = checkedBase64(lines[3], 'updater signature comment');
  if (signed.length !== 74 || global.length !== 64 || signed.subarray(0, 2).toString() !== 'ED') throw new Error('a prehashed updater signature is required');
  if (!signed.subarray(2, 10).equals(key.subarray(2, 10))) throw new Error('updater signature key does not match the configured production key');
  const hash = createHash('blake2b512');
  for await (const bytes of createReadStream(file)) hash.update(bytes);
  const edKey = createPublicKey({ key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), key.subarray(10)]), format: 'der', type: 'spki' });
  const sig = signed.subarray(10);
  if (!verify(null, hash.digest(), edKey, sig) || !verify(null, Buffer.concat([sig, Buffer.from(lines[2].slice(17))]), edKey, global)) throw new Error(`invalid updater signature: ${basename(file)}`);
  // The updater consumes a base64-wrapped document even when the operator
  // supplied a plain Minisign document to this verification boundary.
  return Buffer.from(lines.join('\n') + '\n').toString('base64');
}

async function filesWithin(directory) {
  const result = [];
  const stat = await lstat(directory);
  if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error(`expected an ordinary artifact directory: ${basename(directory)}`);
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isSymbolicLink()) throw new Error(`release artifact must not be a symlink: ${entry.name}`);
    if (entry.isDirectory()) result.push(...await filesWithin(path));
    else if (entry.isFile()) result.push(path);
    else throw new Error(`unsupported release artifact: ${entry.name}`);
  }
  return result;
}

function exactlyOne(files, pattern, label, optional = false) {
  const matches = files.filter(file => pattern.test(basename(file)));
  if (optional && matches.length === 0) return null;
  if (matches.length !== 1) throw new Error(`${label}: expected one artifact, found ${matches.length}`);
  return matches[0];
}

async function digest(file) {
  const hash = createHash('sha256');
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  return hash.digest('hex');
}

export async function stageRelease({ artifacts, output, version, tag, repository = 'seismicgear/annex', publicKey, unsignedDryRun = false, sourceCommit, publishedAt = new Date().toISOString(), provenance }) {
  if (!/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) throw new Error('invalid release version');
  if (!/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repository)) throw new Error('invalid release repository');
  if (!/^[A-Za-z0-9_.-]+$/.test(tag) || (!tag.startsWith('dry-run-') && tag !== `v${version}`)) throw new Error('release tag and version disagree');
  if (!/^[a-f0-9]{40}$/.test(sourceCommit)) throw new Error('exact source commit is required');
  if (unsignedDryRun && !tag.startsWith('dry-run-')) throw new Error('unsigned mode is only permitted for an explicit dry-run draft');
  if (!unsignedDryRun && !publicKey) throw new Error('production updater public key is required');
  const root = resolve(artifacts);
  const out = resolve(output);
  if (out === root || out.startsWith(root + '/') || out.startsWith(root + '\\')) throw new Error('release output must be outside downloaded artifacts');
  await mkdir(out, { recursive: true });
  if ((await readdir(out)).length) throw new Error('release output must be empty; refusing to mix builds');
  const base = `https://github.com/${repository}/releases/download/${tag}`;
  const copied = new Map();
  const downloads = {};
  const updates = {};
  async function stage(file, name) {
    if (!/^[A-Za-z0-9_.+-]+$/.test(name)) throw new Error(`unsafe release asset name: ${name}`);
    const size = (await lstat(file)).size;
    if (size === 0) throw new Error(`empty release artifact: ${name}`);
    if (copied.has(name)) {
      if (copied.get(name) !== file) throw new Error(`release asset collision: ${name}`);
      return;
    }
    await copyFile(file, join(out, name));
    copied.set(name, file);
  }
  for (const platform of platforms) {
    const files = await filesWithin(join(root, platform.group));
    const installer = exactlyOne(files, platform.installer, platform.key);
    await stage(installer, platform.name);
    downloads[platform.key] = { name: platform.name, url: `${base}/${platform.name}`, size: (await lstat(installer)).size, sha256: await digest(installer) };
    for (const extra of platform.optional) {
      const file = exactlyOne(files, extra.pattern, `${platform.key} ${extra.name}`, true);
      if (!file) continue;
      await stage(file, extra.name);
      if (extra.name.endsWith('.msi') && !unsignedDryRun) {
        const signature = await readFile(`${file}.sig`, 'utf8');
        await verifyUpdaterSignature(file, signature, publicKey);
        await stage(`${file}.sig`, `${extra.name}.sig`);
      }
    }
    if (!unsignedDryRun) {
      const updater = exactlyOne(files, platform.updater, `${platform.key} updater`);
      const signature = await readFile(`${updater}.sig`, 'utf8');
      const verifiedSignature = await verifyUpdaterSignature(updater, signature, publicKey);
      await stage(updater, platform.updateName);
      await stage(`${updater}.sig`, `${platform.updateName}.sig`);
      updates[platform.key] = { signature: verifiedSignature, url: `${base}/${platform.updateName}` };
    }
  }
  // Server archives and dependency/provenance inventories remain part of the
  // release. Rename per-crate SBOMs to avoid flat GitHub asset-name collisions.
  for (const file of await filesWithin(root)) {
    if (/^annex-server-.*\.tar\.gz$/.test(basename(file))) await stage(file, basename(file));
    if (file.replaceAll('\\', '/').includes('/sbom/') && file.endsWith('.json')) {
      const name = basename(dirname(file)) === 'rust' ? `rust-${basename(file)}` : basename(file);
      await stage(file, name);
    }
  }
  if (provenance) for (const file of await filesWithin(resolve(provenance))) await stage(file, `zk-${basename(file)}`);
  if (!unsignedDryRun) await writeFile(join(out, 'latest.json'), JSON.stringify({ version, notes: `Annex ${version}`, pub_date: publishedAt, platforms: updates }, null, 2) + '\n');
  await writeFile(join(out, 'downloads.json'), JSON.stringify({ version, tag, sourceCommit, unsignedDryRun, platforms: downloads }, null, 2) + '\n');
  const sums = [];
  for (const name of (await readdir(out)).sort()) sums.push(`${await digest(join(out, name))}  ${name}`);
  await writeFile(join(out, 'SHA256SUMS'), sums.join('\n') + '\n');
  return { version, tag, sourceCommit, platforms: Object.keys(downloads), signed: !unsignedDryRun, assets: await readdir(out) };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = {};
  for (let i = 2; i < process.argv.length; i += 2) {
    if (!process.argv[i].startsWith('--') || !process.argv[i + 1]) throw new Error('arguments require --name value pairs');
    args[process.argv[i].slice(2)] = process.argv[i + 1];
  }
  try {
    const result = await stageRelease({ artifacts: args.artifacts ?? 'artifacts', output: args.output ?? 'release-assets', version: args.version, tag: args.tag, repository: args.repository, publicKey: process.env.TAURI_SIGNING_PUBLIC_KEY, sourceCommit: args.commit ?? process.env.GITHUB_SHA, unsignedDryRun: args['unsigned-dry-run'] === 'true', provenance: args.provenance });
    console.log(JSON.stringify(result, null, 2));
  } catch (error) {
    console.error(`Release staging refused: ${error.message}`);
    process.exitCode = 1;
  }
}
