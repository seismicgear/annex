#!/usr/bin/env node
// Tauri CLI 2.11.2 automatically signs AppImage but not DEB. The vendored
// updater supports DEB and must receive its own authenticated package format.
import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { lstat, readFile, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateInventory } from './array-release.mjs';
import { verifyUpdaterSignature } from './stage-desktop-release.mjs';

async function sha(file) {
  const hash = createHash('sha256');
  for await (const bytes of createReadStream(file)) hash.update(bytes);
  return hash.digest('hex');
}

function officialSigner(file) {
  assert.ok(process.env.TAURI_SIGNING_PRIVATE_KEY, 'Approved updater signing authority required');
  const cli = fileURLToPath(new URL('../tools/tauri-cli/node_modules/@tauri-apps/cli/tauri.js', import.meta.url));
  try {
    // The existing release key/password are read from environment by the
    // official signer, never interpolated into arguments or printed.
    execFileSync(process.execPath, [cli, 'signer', 'sign', file], { stdio: 'pipe', windowsHide: true });
  } catch {
    throw new Error('Official Tauri DEB signing failed; no publication attempted');
  }
}

export async function signDebUpdater({ bundle, version, sourceCommit, publicKey, signFile = officialSigner }) {
  const inventory = validateInventory(JSON.parse(await readFile(join(bundle, 'array-installation.json'), 'utf8')),
    { sourceCommit, releaseVersion: version });
  assert.equal(inventory.platform, 'linux');
  assert.equal(inventory.arch, 'x64');
  const deb = inventory.artifacts.find(row => row.kind === 'deb');
  assert.ok(deb, 'Exact DEB inventory required');
  const filename = resolve(bundle, 'deb', `Annex_${version}_amd64.deb`);
  const stat = await lstat(filename);
  assert.ok(stat.isFile() && !stat.isSymbolicLink());
  assert.equal(stat.size, deb.size);
  assert.equal(await sha(filename), deb.sha256, 'DEB differs from verified native inventory');
  assert.ok(publicKey, 'Production verification public key required');
  const signaturePath = filename + '.sig';
  const signature = await lstat(signaturePath).catch(error => {
    if (error.code === 'ENOENT') return null;
    throw error;
  });
  if (signature) assert.ok(signature.isFile() && !signature.isSymbolicLink() && signature.size < 16384);
  else await signFile(filename);
  await verifyUpdaterSignature(filename, await readFile(signaturePath, 'utf8'), publicKey);
  assert.equal(await sha(filename), deb.sha256, 'Signing changed native payload bytes');
  const receipt = {
    schemaVersion: 1, product: 'annex', platform: 'linux', arch: 'x64', version, sourceCommit,
    name: deb.name, sha256: deb.sha256, size: deb.size, signatureSha256: await sha(signaturePath),
    payloadBytesUnchanged: true, signer: '@tauri-apps/cli@2.11.2',
  };
  await writeFile(join(bundle, 'updater-variants.json'), JSON.stringify(receipt, null, 2) + '\n');
  return receipt;
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = {};
  for (let i = 2; i < process.argv.length; i += 2) {
    assert.ok(['--bundle', '--version', '--commit'].includes(process.argv[i]) && process.argv[i + 1]);
    args[process.argv[i].slice(2)] = process.argv[i + 1];
  }
  try {
    const receipt = await signDebUpdater({ bundle: args.bundle, version: args.version,
      sourceCommit: args.commit, publicKey: process.env.TAURI_SIGNING_PUBLIC_KEY });
    console.log(JSON.stringify(receipt));
  } catch (error) {
    console.error(`[deb-updater-signing] ${error.message}`);
    process.exitCode = 1;
  }
}
