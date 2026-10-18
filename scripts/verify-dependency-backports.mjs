// Reconstruct each reviewed backport from a checksum-pinned crates.io archive.
// This must pass with resolved Cargo metadata before an advisory disposition.
import fs from 'node:fs/promises';
import { constants } from 'node:fs';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { gunzipSync } from 'node:zlib';
import { fileURLToPath, pathToFileURL } from 'node:url';

const repo = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const sha256 = data => createHash('sha256').update(data).digest('hex');
function safePath(root, relative) {
  if (!relative || relative.includes('\\') || relative.includes(':') || relative.startsWith('/') || relative.split('/').some(part => !part || part === '.' || part === '..')) throw new Error(`Unsafe backport path: ${relative}`);
  return path.join(root, relative);
}
function tarFiles(archive, prefix) {
  const bytes = gunzipSync(archive, { maxOutputLength: 32 * 1024 * 1024 });
  const files = new Map();
  let longName = null;
  for (let offset = 0; offset + 512 <= bytes.length;) {
    const header = bytes.subarray(offset, offset + 512);
    if (header.every(value => value === 0)) break;
    const string = (from, length) => header.subarray(from, from + length).toString('utf8').replace(/\0.*$/s, '');
    const directory = string(345, 155);
    const headerName = `${directory ? `${directory}/` : ''}${string(0, 100)}`;
    const sizeText = string(124, 12).trim();
    if (!/^[0-7]+$/.test(sizeText)) throw new Error('Invalid crate entry length');
    const size = parseInt(sizeText, 8);
    if (offset + 512 + size > bytes.length) throw new Error('Truncated crate entry');
    const payload = bytes.subarray(offset + 512, offset + 512 + size);
    // crates.io uses GNU long-name records for source paths over 100 bytes.
    // They describe exactly the next regular file; never accept link records.
    if (header[156] === 76) {
      if (longName !== null || headerName !== '././@LongLink' || size < 2 || size > 4096 || payload[size - 1] !== 0 || payload.subarray(0, -1).includes(0)) throw new Error('Invalid crate long-name record');
      longName = payload.subarray(0, -1).toString('utf8');
      offset += 512 + Math.ceil(size / 512) * 512;
      continue;
    }
    const name = longName ?? headerName;
    longName = null;
    if (!name.startsWith(prefix) || header[156] !== 48) throw new Error(`Unsupported entry in pinned crate: ${name}`);
    const relative = name.slice(prefix.length); safePath('.', relative);
    if (files.has(relative) || offset + 512 + size > bytes.length) throw new Error(`Duplicate or truncated crate entry: ${relative}`);
    files.set(relative, bytes.subarray(offset + 512, offset + 512 + size));
    offset += 512 + Math.ceil(size / 512) * 512;
  }
  if (longName !== null) throw new Error('Dangling crate long-name record');
  if (!files.size) throw new Error('Pinned crate contains no files');
  return files;
}
export async function regularFile(filename) {
  const handle = await fs.open(filename, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  try {
    const opened = await handle.stat({ bigint: true });
    // Windows does not expose O_NOFOLLOW through Node. Reject links and require
    // the current directory entry to identify this exact opened object there
    // too. The subsequent read never reopens the replaceable pathname.
    const entry = await fs.lstat(filename, { bigint: true });
    if (!opened.isFile() || !entry.isFile() || entry.isSymbolicLink()
      || opened.dev !== entry.dev || opened.ino !== entry.ino) {
      throw new Error(`Backport file must be the same regular file: ${filename}`);
    }
    return await handle.readFile();
  } finally {
    await handle.close();
  }
}
async function filePaths(directory, prefix = '') {
  const paths = [];
  for (const entry of await fs.readdir(directory, { withFileTypes: true })) {
    const relative = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.isDirectory()) paths.push(...await filePaths(path.join(directory, entry.name), relative));
    else if (entry.isFile()) paths.push(relative);
    else throw new Error(`Unsupported backport filesystem entry: ${relative}`);
  }
  return paths.sort();
}
export async function verifyBackports(root = repo, metadata) {
  const manifest = JSON.parse(await fs.readFile(path.join(root, 'scripts/dependency-backports.json'), 'utf8'));
  if (manifest.version !== 1) throw new Error('Unsupported dependency-backport manifest');
  for (const entry of manifest.crates) {
    const archive = await regularFile(safePath(root, entry.archive.path));
    if (archive.length !== entry.archive.bytes || sha256(archive) !== entry.archive.sha256) throw new Error(`Upstream archive checksum mismatch: ${entry.name}`);
    const originals = tarFiles(archive, `${entry.name}-${entry.version}/`);
    const changed = new Map();
    for (const patch of entry.patches) {
      const original = originals.get(patch.path);
      if (!original || sha256(original) !== patch.original_sha256) throw new Error(`Patch preimage mismatch: ${entry.name}/${patch.path}`);
      let expected = original.toString('utf8');
      for (const replacement of patch.replacements) {
        if (!replacement.before || expected.split(replacement.before).length !== 2) throw new Error(`Patch is not unique: ${entry.name}/${patch.path}`);
        expected = expected.replace(replacement.before, replacement.after);
      }
      const bytes = Buffer.from(expected);
      if (sha256(bytes) !== patch.patched_sha256) throw new Error(`Patch result mismatch: ${entry.name}/${patch.path}`);
      changed.set(patch.path, bytes);
    }
    const destination = safePath(root, entry.path);
    const actualPaths = await filePaths(destination);
    if (JSON.stringify(actualPaths) !== JSON.stringify([...originals.keys()].sort())) throw new Error(`Vendored crate file inventory changed: ${entry.name}`);
    for (const [relative, original] of originals) {
      const actual = await regularFile(safePath(destination, relative));
      if (!actual.equals(changed.get(relative) ?? original)) throw new Error(`Vendored source differs from exact reviewed backport: ${entry.name}/${relative}`);
    }
    if (metadata) {
      const packages = metadata.packages.filter(pkg => pkg.name === entry.name && pkg.version === entry.version);
      const expectedManifest = await fs.realpath(path.join(destination, 'Cargo.toml'));
      if (packages.length !== 1 || packages[0].source !== null || await fs.realpath(packages[0].manifest_path) !== expectedManifest) throw new Error(`Cargo did not resolve the verified vendored crate: ${entry.name}`);
      if (entry.name === 'glib') {
        for (const pkg of metadata.packages.filter(pkg => pkg.name === 'glib')) {
          const [major, minor] = pkg.version.split('.').map(Number);
          if (major === 0 && minor >= 15 && minor < 20 && pkg.id !== packages[0].id) throw new Error('Cargo also resolves another GLib version affected by RUSTSEC-2024-0429');
        }
      }
      if (entry.name === 'ark-relations') {
        const node = metadata.resolve?.nodes.find(node => node.id === packages[0].id);
        const subscriber = metadata.packages.find(pkg => pkg.name === 'tracing-subscriber' && node?.dependencies.includes(pkg.id));
        const parts = subscriber?.version.split('.').map(Number);
        if (!parts || parts[0] !== 0 || parts[1] !== 3 || parts[2] < 23) throw new Error('Ark does not resolve the repaired tracing subscriber');
      }
      if (entry.name === 'tauri-plugin-updater') {
        const node = metadata.resolve?.nodes.find(node => node.id === packages[0].id);
        const archives = metadata.packages.filter(pkg => pkg.name === 'zip');
        if (archives.length !== 1 || archives[0].version !== '9.0.3' || !node?.dependencies.includes(archives[0].id)) throw new Error('Updater must resolve only the reviewed supported ZIP reader');
      }
      if (entry.name === 'tauri-plugin-single-instance') {
        const node = metadata.resolve?.nodes.find(node => node.id === packages[0].id);
        const buses = metadata.packages.filter(pkg => pkg.name === 'zbus');
        const retired = metadata.packages.filter(pkg => ['zbus_names', 'zvariant', 'zvariant_derive'].includes(pkg.name));
        if (buses.length !== 1 || buses[0].version !== '6.0.0' || !node?.dependencies.includes(buses[0].id) || retired.length) throw new Error('Single-instance must resolve maintained zbus without the retired wire/name crates');
      }
    }
  }
  for (const entry of manifest.regression_files) {
    if (sha256(await regularFile(safePath(root, entry.path))) !== entry.sha256) throw new Error(`Backport regression gate changed: ${entry.path}`);
  }
  return { crates: manifest.crates.map(entry => `${entry.name}@${entry.version}`), resolved: !!metadata };
}
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  try {
    const args = process.argv.slice(2);
    let metadata;
    if (args.length === 2 && args[0] === '--metadata') metadata = JSON.parse(await fs.readFile(args[1], 'utf8'));
    else if (args.length !== 1 || args[0] !== '--source-only') throw new Error('Use --metadata <full-cargo-metadata.json> before audit, or --source-only for source inspection');
    const result = await verifyBackports(repo, metadata);
    console.log(`[dependency-backports] exact source verified: ${result.crates.join(', ')}; Cargo resolution ${result.resolved ? 'verified' : 'NOT checked (source-only)'}`);
  } catch (error) { console.error(`[dependency-backports] ${error.message}`); process.exitCode = 1; }
}
