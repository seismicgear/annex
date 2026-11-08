// Populate the exact Windows tool cache consumed by Tauri bundler 2.9.2.
// These build tools are separate from Annex's shipped native runtime assets.
import fs from 'node:fs/promises';
import path from 'node:path';
import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { inflateRawSync } from 'node:zlib';

const hash = (bytes, algorithm = 'sha256') => createHash(algorithm).update(bytes).digest('hex');
const pinPath = new URL('./nsis-tools.json', import.meta.url);
const reserved = /^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/i;
function relativePath(value) {
  const parts = value.split('/');
  if (!value || /[\\:\x00-\x1f]/.test(value) || parts.some(p => !p || p === '.' || p === '..' || /[. ]$/.test(p) || reserved.test(p))) {
    throw new Error(`Unsafe NSIS path: ${value}`);
  }
  return value;
}
async function statIfPresent(file) {
  try { return await fs.lstat(file); } catch (error) { if (error.code === 'ENOENT') return null; throw error; }
}
// Inspect each existing ancestor, rather than following a cache symlink/junction.
async function safeDirectory(directory, create = false) {
  const absolute = path.resolve(directory);
  const parsed = path.parse(absolute);
  let current = parsed.root;
  for (const component of absolute.slice(parsed.root.length).split(path.sep).filter(Boolean)) {
    current = path.join(current, component);
    let info = await statIfPresent(current);
    if (!info && create) {
      try { await fs.mkdir(current); } catch (error) { if (error.code !== 'EEXIST') throw error; }
      info = await fs.lstat(current);
    }
    if (info && (!info.isDirectory() || info.isSymbolicLink())) throw new Error(`Unsafe NSIS cache directory: ${current}`);
  }
  return absolute;
}
async function safeFile(file) {
  await safeDirectory(path.dirname(file));
  const info = await statIfPresent(file);
  if (info && (!info.isFile() || info.isSymbolicLink())) throw new Error(`Unsafe NSIS cache file: ${file}`);
  return info;
}
const crcTable = Array.from({ length: 256 }, (_, i) => {
  for (let bit = 0; bit < 8; bit++) i = (i >>> 1) ^ ((i & 1) ? 0xedb88320 : 0);
  return i >>> 0;
});
function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) crc = (crc >>> 8) ^ crcTable[(crc ^ byte) & 0xff];
  return (crc ^ 0xffffffff) >>> 0;
}

// The pinned, small NSIS ZIP uses ordinary stored/deflated entries. Parse its
// central directory first; reject links, aliases, traversal and ZIP64 before
// writing anything. No platform extraction utility or new package is needed.
function readToolsetZip(bytes, root) {
  let end = -1;
  for (let i = bytes.length - 22; i >= Math.max(0, bytes.length - 65557); i--) {
    if (bytes.readUInt32LE(i) === 0x06054b50 && i + 22 + bytes.readUInt16LE(i + 20) === bytes.length) { end = i; break; }
  }
  if (end < 0) throw new Error('Invalid NSIS ZIP end record');
  const count = bytes.readUInt16LE(end + 10);
  const centralBytes = bytes.readUInt32LE(end + 12);
  const centralOffset = bytes.readUInt32LE(end + 16);
  if (bytes.readUInt16LE(end + 4) || bytes.readUInt16LE(end + 6) || bytes.readUInt16LE(end + 8) !== count ||
      !count || count > 10000 || centralOffset + centralBytes !== end) throw new Error('Unsupported NSIS ZIP directory');
  const files = new Map();
  const paths = new Map();
  const ranges = [];
  let cursor = centralOffset;
  let totalBytes = 0;
  for (let index = 0; index < count; index++) {
    if (cursor + 46 > end || bytes.readUInt32LE(cursor) !== 0x02014b50) throw new Error('Invalid NSIS ZIP entry');
    const flags = bytes.readUInt16LE(cursor + 8), method = bytes.readUInt16LE(cursor + 10);
    const crc = bytes.readUInt32LE(cursor + 16), compressed = bytes.readUInt32LE(cursor + 20), size = bytes.readUInt32LE(cursor + 24);
    const nameSize = bytes.readUInt16LE(cursor + 28), extraSize = bytes.readUInt16LE(cursor + 30), commentSize = bytes.readUInt16LE(cursor + 32);
    const attributes = bytes.readUInt32LE(cursor + 38), local = bytes.readUInt32LE(cursor + 42);
    const next = cursor + 46 + nameSize + extraSize + commentSize;
    if (next > end || !nameSize || bytes.readUInt16LE(cursor + 34) || (flags & ~0x080e) || ![0, 8].includes(method) ||
        compressed === 0xffffffff || size === 0xffffffff || local === 0xffffffff) throw new Error('Unsupported NSIS ZIP entry');
    const rawName = bytes.subarray(cursor + 46, cursor + 46 + nameSize);
    const name = rawName.toString('utf8');
    if (!Buffer.from(name).equals(rawName)) throw new Error('Invalid NSIS ZIP filename encoding');
    const directory = name.endsWith('/');
    const clean = relativePath(directory ? name.slice(0, -1) : name);
    const unixType = (attributes >>> 16) & 0xf000;
    if ((unixType && unixType !== (directory ? 0x4000 : 0x8000)) || (!directory && (attributes & 0x10))) throw new Error('NSIS ZIP links or special entries are forbidden');
    if (clean !== root && !clean.startsWith(root + '/')) throw new Error('Unexpected NSIS ZIP root');
    if (clean === root && !directory) throw new Error('NSIS ZIP root must be a directory');
    const key = clean.toLowerCase();
    if (paths.has(key)) throw new Error('Duplicate or case-aliased NSIS ZIP path');
    paths.set(key, { clean, directory });
    if (local + 30 > centralOffset || bytes.readUInt32LE(local) !== 0x04034b50 ||
        bytes.readUInt16LE(local + 6) !== flags || bytes.readUInt16LE(local + 8) !== method) throw new Error('Invalid NSIS ZIP local header');
    const localNameSize = bytes.readUInt16LE(local + 26), localExtraSize = bytes.readUInt16LE(local + 28);
    const dataStart = local + 30 + localNameSize + localExtraSize;
    if (localNameSize !== nameSize || !bytes.subarray(local + 30, local + 30 + localNameSize).equals(rawName) ||
        dataStart + compressed > centralOffset) throw new Error('Inconsistent NSIS ZIP local entry');
    if (!(flags & 8) && (bytes.readUInt32LE(local + 14) !== crc || bytes.readUInt32LE(local + 18) !== compressed || bytes.readUInt32LE(local + 22) !== size)) throw new Error('Inconsistent NSIS ZIP sizes');
    ranges.push([local, dataStart + compressed]);
    totalBytes += size;
    if (totalBytes > 128 * 1024 * 1024) throw new Error('NSIS ZIP exceeds the extraction limit');
    const packed = bytes.subarray(dataStart, dataStart + compressed);
    const content = method === 8 ? inflateRawSync(packed, { maxOutputLength: size + 1 }) : packed;
    if (content.length !== size || crc32(content) !== crc || (directory && size)) throw new Error('NSIS ZIP entry integrity failed');
    if (!directory) files.set(clean.slice(root.length + 1), content);
    cursor = next;
  }
  if (cursor !== end) throw new Error('Unexpected NSIS ZIP directory data');
  ranges.sort((a, b) => a[0] - b[0]);
  if (ranges.some((row, i) => i && row[0] < ranges[i - 1][1])) throw new Error('Overlapping NSIS ZIP entries');
  for (const { clean } of paths.values()) {
    const parts = clean.split('/');
    for (let i = 1; i < parts.length; i++) {
      const parent = paths.get(parts.slice(0, i).join('/').toLowerCase());
      if (parent && !parent.directory) throw new Error('NSIS ZIP path crosses a file');
    }
  }
  return files;
}

async function cacheInventory(directory) {
  const files = new Map();
  async function walk(current, prefix = '') {
    for (const entry of await fs.readdir(current, { withFileTypes: true })) {
      const relative = relativePath(prefix + entry.name);
      const file = path.join(current, entry.name);
      if (entry.isSymbolicLink()) throw new Error(`Unsafe NSIS cache link: ${relative}`);
      if (entry.isDirectory()) await walk(file, relative + '/');
      else if (entry.isFile()) {
        const key = relative.toLowerCase();
        if (files.has(key)) throw new Error('Case-aliased NSIS cache files');
        const data = await fs.readFile(file);
        files.set(key, { path: relative, bytes: data.length, sha256: hash(data) });
      } else throw new Error(`Unsafe NSIS cache entry: ${relative}`);
    }
  }
  await walk(directory);
  return files;
}

// Exported for real TLS/archive/cache fixtures. The production entry below
// supplies only the committed pins and the Windows known-folder cache path.
export async function prepareNsisCache({ pins, toolsDirectory, archiveDirectory, download }) {
  const cache = await safeDirectory(archiveDirectory, true);
  const tools = await safeDirectory(toolsDirectory);
  const destination = path.join(tools, 'NSIS');
  await safeDirectory(destination);
  const downloaded = {};
  for (const kind of ['toolset', 'plugin']) {
    const pin = pins[kind];
    const file = path.join(cache, relativePath(pin.name));
    await safeFile(file);
    await download(pin.url, file, pin);
    await safeFile(file);
    const data = await fs.readFile(file);
    if (data.length !== pin.bytes || hash(data) !== pin.sha256 || hash(data, 'sha1') !== pin.upstreamSha1) throw new Error(`Pinned NSIS ${kind} integrity failed`);
    downloaded[kind] = data;
  }
  const files = readToolsetZip(downloaded.toolset, relativePath(pins.toolset.root));
  const pluginPath = relativePath(pins.plugin.path);
  if (files.has(pluginPath)) throw new Error('NSIS toolset unexpectedly contains the separately pinned plugin');
  files.set(pluginPath, downloaded.plugin);
  for (const required of pins.requiredFiles) {
    if (!files.get(relativePath(required))?.length) throw new Error(`Incomplete pinned NSIS toolset: ${required}`);
  }
  const expected = [...files].map(([file, bytes]) => ({ path: file, bytes: bytes.length, sha256: hash(bytes) })).sort((a, b) => a.path.localeCompare(b.path));
  const present = await statIfPresent(destination) ? await cacheInventory(destination) : new Map();
  // Finish the entire preflight before adding any file to an existing cache.
  for (const row of expected) {
    await safeFile(path.join(destination, row.path));
    const prior = present.get(row.path.toLowerCase());
    if (prior && (prior.path !== row.path || prior.bytes !== row.bytes || prior.sha256 !== row.sha256)) throw new Error(`NSIS cache differs from pinned toolset: ${row.path}`);
  }
  await safeDirectory(tools, true);
  const staging = await fs.mkdtemp(path.join(tools, '.annex-nsis-stage-'));
  try {
    const stagedToolset = path.join(staging, 'NSIS');
    await fs.mkdir(stagedToolset);
    for (const [relative, content] of files) {
      const file = path.join(stagedToolset, relative);
      await fs.mkdir(path.dirname(file), { recursive: true });
      await fs.writeFile(file, content, { flag: 'wx' });
    }
    if (!await statIfPresent(destination)) {
      await fs.rename(stagedToolset, destination);
    } else {
      for (const row of expected) {
        if (present.has(row.path.toLowerCase())) continue;
        const output = path.join(destination, row.path);
        await safeDirectory(path.dirname(output), true);
        // A hard link publishes a complete staged file without overwriting a
        // concurrent file. Staging and the Tauri cache share the same volume.
        await fs.link(path.join(stagedToolset, row.path), output);
      }
    }
    const actual = await cacheInventory(destination);
    for (const row of expected) {
      if (JSON.stringify(actual.get(row.path.toLowerCase())) !== JSON.stringify(row)) throw new Error(`NSIS cache verification failed: ${row.path}`);
    }
    const receipt = { schemaVersion: 1, tauri: pins.tauri, toolset: pins.toolset, plugin: pins.plugin,
      toolsetDirectory: destination, files: expected };
    const receiptBytes = Buffer.from(JSON.stringify(receipt, null, 2) + '\n');
    const receiptFile = path.join(cache, `verified-${hash(receiptBytes)}.json`);
    if (await safeFile(receiptFile)) {
      if (!(await fs.readFile(receiptFile)).equals(receiptBytes)) throw new Error('NSIS receipt collision');
    } else await fs.writeFile(receiptFile, receiptBytes, { flag: 'wx' });
    console.log(`[nsis-tools] verified ${expected.length} pinned files for Tauri CLI ${pins.tauri.cliVersion} / bundler ${pins.tauri.bundlerVersion}; receipt: ${receiptFile}`);
    return receipt;
  } finally {
    // Remove only this invocation's private staging directory, never NSIS or
    // another cache entry. Unrelated existing tool-cache files are preserved.
    await fs.rm(staging, { recursive: true, force: true });
  }
}

export function windowsTauriToolsDirectory() {
  if (process.platform !== 'win32') throw new Error('NSIS cache preparation requires a Windows host');
  // dirs 6 (used by Tauri) resolves FOLDERID_LocalAppData. The .NET special
  // folder API resolves that same Windows known folder, including redirection.
  const local = execFileSync('powershell.exe', ['-NoLogo', '-NoProfile', '-NonInteractive', '-Command',
    "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); [Environment]::GetFolderPath([Environment+SpecialFolder]::LocalApplicationData)"],
  { encoding: 'utf8', windowsHide: true }).replace(/\r?\n$/, '');
  if (!path.win32.isAbsolute(local) || /[\r\n\0]/.test(local)) throw new Error('Windows LocalApplicationData known folder is unavailable');
  return path.join(local, 'tauri');
}

export async function prepareNsisTools({ repositoryRoot, workDirectory, download }) {
  const pins = JSON.parse(await fs.readFile(pinPath, 'utf8'));
  const cli = JSON.parse(await fs.readFile(path.join(repositoryRoot, 'tools/tauri-cli/package.json'), 'utf8'));
  const config = JSON.parse(await fs.readFile(path.join(repositoryRoot, 'crates/annex-desktop/tauri.conf.json'), 'utf8'));
  if (pins.schemaVersion !== 1 || cli.dependencies?.['@tauri-apps/cli'] !== pins.tauri.cliVersion || config.bundle?.useLocalToolsDir) {
    throw new Error('NSIS cache pins require the reviewed Tauri CLI and default Windows tool-cache configuration');
  }
  return prepareNsisCache({ pins, toolsDirectory: windowsTauriToolsDirectory(),
    archiveDirectory: path.join(workDirectory, 'nsis-tools'), download });
}
