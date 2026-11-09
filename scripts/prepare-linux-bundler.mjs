#!/usr/bin/env node
// Pin the build-only tools consumed by Tauri CLI 2.11.2's AppImage bundler.
import fs from 'node:fs/promises';
import path from 'node:path';
import os from 'node:os';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { download, verifyFile } from './prepare-desktop-assets.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const requiredNames = ['AppRun-x86_64', 'linuxdeploy-x86_64.AppImage',
  'linuxdeploy-plugin-appimage.AppImage', 'linuxdeploy-plugin-gtk.sh', 'linuxdeploy-plugin-gstreamer.sh'];

async function statIfPresent(file) {
  try { return await fs.lstat(file); } catch (error) { if (error.code === 'ENOENT') return null; throw error; }
}
async function safeDirectory(directory) {
  const absolute = path.resolve(directory);
  const parsed = path.parse(absolute);
  let current = parsed.root;
  for (const component of absolute.slice(parsed.root.length).split(path.sep).filter(Boolean)) {
    current = path.join(current, component);
    try { await fs.mkdir(current); } catch (error) { if (error.code !== 'EEXIST') throw error; }
    const stat = await fs.lstat(current);
    if (!stat.isDirectory() || stat.isSymbolicLink()) throw new Error(`Unsafe Linux bundler directory: ${current}`);
  }
  return absolute;
}
async function safeFile(file) {
  const stat = await statIfPresent(file);
  if (stat && (!stat.isFile() || stat.isSymbolicLink())) throw new Error(`Unsafe Linux bundler file: ${file}`);
}

export async function prepareLinuxBundler({ pins, toolsDirectory, archiveDirectory, downloader = download }) {
  if (pins.schemaVersion !== 1 || pins.tauri?.cliVersion !== '2.11.2' || pins.target !== 'x86_64-unknown-linux-gnu' ||
      JSON.stringify(pins.files?.map(pin => pin.name).sort()) !== JSON.stringify([...requiredNames].sort())) {
    throw new Error('Linux bundler pins do not match the reviewed Tauri toolset');
  }
  for (const pin of pins.files) {
    if (!Number.isSafeInteger(pin.bytes) || pin.bytes <= 0 || !/^[a-f0-9]{64}$/.test(pin.sha256) || new URL(pin.url).protocol !== 'https:') {
      throw new Error(`Invalid Linux bundler pin: ${pin.name}`);
    }
  }
  const archive = await safeDirectory(archiveDirectory);
  const tools = await safeDirectory(toolsDirectory);
  if (archive === tools || archive.startsWith(tools + path.sep) || tools.startsWith(archive + path.sep)) {
    throw new Error('Verified downloads must be separate from the mutable Tauri tool cache');
  }
  for (const pin of pins.files) await safeFile(path.join(tools, pin.name));
  const staging = await fs.mkdtemp(path.join(tools, '.annex-linux-stage-'));
  try {
    // Tauri clears bytes 8..10 in linuxdeploy when bundling. Keep the original
    // verified download separate, then restore a fresh executable on every run.
    // Verify the entire toolset before changing any executable in Tauri's cache.
    for (const pin of pins.files) {
      const original = path.join(archive, `${pin.sha256}-${pin.name}`);
      await safeFile(original);
      await downloader(pin.url, original, pin);
      await safeFile(original);
      await verifyFile(original, pin);
      const staged = path.join(staging, pin.name);
      await fs.copyFile(original, staged);
      await verifyFile(staged, pin);
      await fs.chmod(staged, 0o755);
    }
    for (const pin of pins.files) {
      const destination = path.join(tools, pin.name);
      await safeFile(destination);
      await fs.rename(path.join(staging, pin.name), destination);
    }
    const receipt = { schemaVersion: 1, tauri: pins.tauri, target: pins.target, toolsDirectory: tools, files: pins.files };
    await fs.writeFile(path.join(archive, 'verified-linux-bundler.json'), JSON.stringify(receipt, null, 2) + '\n');
    return receipt;
  } finally {
    await fs.rm(staging, { recursive: true, force: true });
  }
}

export async function run() {
  if (process.platform !== 'linux' || process.arch !== 'x64') throw new Error('Linux bundler preparation requires a native Linux x86_64 host');
  if (process.argv.length !== 2) throw new Error('Usage: node scripts/prepare-linux-bundler.mjs (set XDG_CACHE_HOME to a job-owned cache)');
  const pins = JSON.parse(await fs.readFile(path.join(root, 'scripts/linux-bundler-tools.json'), 'utf8'));
  const cli = JSON.parse(await fs.readFile(path.join(root, 'tools/tauri-cli/package.json'), 'utf8'));
  const config = JSON.parse(await fs.readFile(path.join(root, 'crates/annex-desktop/tauri.conf.json'), 'utf8'));
  if (cli.dependencies?.['@tauri-apps/cli'] !== pins.tauri.cliVersion || config.bundle?.useLocalToolsDir) {
    throw new Error('Linux bundler preparation requires the pinned CLI and default Tauri tool-cache configuration');
  }
  const cache = process.env.XDG_CACHE_HOME || path.join(os.homedir(), '.cache');
  if (!path.isAbsolute(cache)) throw new Error('XDG_CACHE_HOME must be an absolute path');
  const receipt = await prepareLinuxBundler({ pins, toolsDirectory: path.join(cache, 'tauri'), archiveDirectory: path.join(cache, 'annex-linux-bundler-downloads') });
  console.log(`[linux-bundler] verified ${receipt.files.length} pinned tools for Tauri CLI ${pins.tauri.cliVersion}: ${receipt.toolsDirectory}`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  run().catch(error => { console.error(error.message); process.exitCode = 1; });
}
