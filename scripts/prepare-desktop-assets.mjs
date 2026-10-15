#!/usr/bin/env node
// Prepare real, pinned native resources before Tauri reads bundle.resources.
// Does not build the client, run Cargo, install Annex, or launch an Annex profile.
import fs from 'node:fs/promises';
import { createReadStream, createWriteStream } from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { createHash, randomUUID } from 'node:crypto';
import { pipeline } from 'node:stream/promises';
import { Readable } from 'node:stream';
import { spawnSync } from 'node:child_process';
import { buildMacPiper } from './build-macos-piper.mjs';
import { preparePinnedGitSource } from './pinned-git-source.mjs';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const pinPath = path.join(root, 'scripts', 'desktop-assets.json');
export const cmakeFlags = [
  '-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_POLICY_VERSION_MINIMUM=3.5',
  '-DCMAKE_POLICY_DEFAULT_CMP0091=NEW', '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded',
  '-DBUILD_SHARED_LIBS=OFF', '-DGGML_BACKEND_DL=OFF', '-DGGML_NATIVE=OFF',
  '-DGGML_OPENMP=OFF', '-DGGML_BLAS=OFF', '-DGGML_ACCELERATE=OFF',
  '-DGGML_METAL=OFF', '-DGGML_CUDA=OFF', '-DGGML_VULKAN=OFF',
  '-DGGML_AVX=OFF', '-DGGML_AVX2=OFF', '-DGGML_FMA=OFF', '-DGGML_F16C=OFF',
  '-DWHISPER_BUILD_TESTS=OFF', '-DWHISPER_BUILD_SERVER=OFF',
  '-DWHISPER_BUILD_EXAMPLES=ON', '-DWHISPER_CURL=OFF', '-DWHISPER_SDL2=OFF',
];
const targets = { 'win32-x64': 'x86_64-pc-windows-msvc', 'linux-x64': 'x86_64-unknown-linux-gnu',
  'darwin-x64': 'x86_64-apple-darwin', 'darwin-arm64': 'aarch64-apple-darwin' };
export function platformFor(target, platform = process.platform, arch = process.arch) {
  const host = `${platform}-${arch}`;
  if (!targets[host] || (target && targets[host] !== target)) {
    throw new Error(`Unsupported or cross-architecture desktop preparation: host=${host}, target=${target ?? 'native'}. Use a matching native runner.`);
  }
  return host;
}
export function checkedPath(base, relative) {
  if (!relative || relative.includes('\\') || relative.includes(':') || relative.startsWith('/') || relative.split('/').some(x => x === '..' || x === '.')) {
    throw new Error(`Unsafe resource path: ${relative}`);
  }
  const resolved = path.resolve(base, relative);
  if (!resolved.startsWith(path.resolve(base) + path.sep)) throw new Error('Resource escapes destination');
  return resolved;
}
export function whisperBuildDirectory(cache, source, generator) {
  const sourceKey = createHash('sha256').update(path.resolve(source)).digest('hex').slice(0, 16);
  const prefix = generator ? `whisper-build-${generator.replace(/[^a-z0-9]+/gi, '-')}` : 'whisper-build';
  // CMake binds a build tree to its original source path. A recovered checkout
  // must not reuse (or remove) another source directory's configured build tree.
  return path.join(cache, `${prefix}-${sourceKey}`);
}
async function digest(filename) {
  const hash = createHash('sha256');
  for await (const part of createReadStream(filename)) hash.update(part);
  return hash.digest('hex');
}
async function exists(filename) { try { await fs.lstat(filename); return true; } catch (e) { if (e.code === 'ENOENT') return false; throw e; } }
export async function verifyFile(filename, pin) {
  const stat = await fs.stat(filename);
  if (!stat.isFile() || stat.size === 0 || (pin.bytes && stat.size !== pin.bytes) || await digest(filename) !== pin.sha256) {
    throw new Error(`Pinned resource verification failed: ${filename}`);
  }
}
async function download(url, filename, pin) {
  if (await exists(filename)) { await verifyFile(filename, pin); return; }
  if (new URL(url).protocol !== 'https:') throw new Error('Resource downloads require HTTPS');
  await fs.mkdir(path.dirname(filename), { recursive: true });
  const temporary = `${filename}.${randomUUID()}.part`;
  try {
    const response = await fetch(url, { signal: AbortSignal.timeout(300_000) });
    if (!response.ok || !response.body || new URL(response.url).protocol !== 'https:') throw new Error(`Resource download failed (${response.status}): ${url}`);
    await pipeline(Readable.fromWeb(response.body), createWriteStream(temporary, { flags: 'wx' }));
    await verifyFile(temporary, pin);
    await fs.rename(temporary, filename);
  } finally { await fs.rm(temporary, { force: true }); }
}
function command(binary, args, cwd, capture = false) {
  const result = spawnSync(binary, args, { cwd, windowsHide: true, encoding: 'utf8',
    stdio: capture ? ['ignore', 'pipe', 'pipe'] : 'inherit', maxBuffer: 32 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${binary} failed (${result.status}): ${capture ? result.stderr : 'see output'}`);
  return (result.stdout ?? '') + (result.stderr ?? '');
}
async function inventory(directory) {
  const rows = [];
  const canonical = await fs.realpath(directory);
  async function walk(current, prefix) {
    for (const entry of await fs.readdir(current, { withFileTypes: true })) {
      const name = prefix ? `${prefix}/${entry.name}` : entry.name;
      if (entry.name === '.gitkeep' || name === 'desktop-provenance.json') continue;
      const file = checkedPath(directory, name);
      const actual = await fs.realpath(file);
      if (!actual.startsWith(canonical + path.sep)) throw new Error(`Resource link escapes bundle: ${name}`);
      if (entry.isDirectory()) await walk(file, name);
      else if (entry.isSymbolicLink() && (await fs.stat(file)).isDirectory()) throw new Error('Directory links are not bundle resources');
      else { const stat = await fs.stat(file); if (!stat.isFile()) throw new Error('Non-file bundle resource'); rows.push({ path: name, bytes: stat.size, sha256: await digest(file) }); }
    }
  }
  await walk(directory, ''); return rows.sort((a,b) => a.path.localeCompare(b.path));
}
async function resourceInventory(assets) {
  const rows=[];
  // These are the five native-resource directories in tauri.conf.json. The
  // repository also has source artwork; that is not a speech runtime input.
  for(const directory of ['embedding','models','piper','voices','whisper']) {
    for(const entry of await inventory(path.join(assets,directory))) rows.push({...entry,path:`${directory}/${entry.path}`});
  }
  return rows.sort((a,b)=>a.path.localeCompare(b.path));
}
async function preparePiper(pins, platform, assets, cache) {
  if (platform.startsWith('darwin-')) {
    return buildMacPiper({ pins, platform, assets, cache, download, command, inventory, checkedPath });
  }
  const pin = pins.piper.archives[platform];
  const archive = path.join(cache, pin.name);
  await download(pins.piper.base_url + pin.name, archive, pin);
  const members = command('tar', ['-tf', archive], cache, true).split(/\r?\n/).filter(Boolean);
  for (const member of members) { checkedPath(cache, member.replace(/\/$/, '')); if (member !== 'piper/' && !member.startsWith('piper/')) throw new Error('Unexpected archive root'); }
  const staging = await fs.mkdtemp(path.join(cache, 'piper-stage-'));
  command('tar', ['-xf', archive, '-C', staging], cache);
  const extracted = path.join(staging, 'piper');
  const files = await inventory(extracted); // Reject escaping links before copying.
  const destination = path.join(assets, 'piper');
  await fs.mkdir(destination, { recursive: true });
  // Do not overwrite a different user's runtime or leave mixed old/new DLLs.
  const present = await inventory(destination);
  if (present.length && JSON.stringify(present) !== JSON.stringify(files)) throw new Error('Piper destination contains a different runtime. Use an empty asset directory.');
  if (!present.length) await fs.cp(extracted, destination, { recursive: true, dereference: true });
  const binary = path.join(destination, platform.startsWith('win32') ? 'piper.exe' : 'piper');
  const result = command(binary, ['--help'], destination, true);
  if (!result.includes('--model') || !result.includes('--output_raw')) throw new Error('Piper runtime is not the expected CLI');
}
async function prepareWhisper(pins, platform, assets, cache) {
  const source = await preparePinnedGitSource({ cache, name: 'whisper-source', ...pins.whisper });
  const actual = pins.whisper.commit; // The source helper verified this exact commit.
  const generator = process.env.CMAKE_GENERATOR;
  const build = whisperBuildDirectory(cache, source, generator);
  const flags = [...cmakeFlags];
  if (platform === 'darwin-x64') flags.push('-DCMAKE_OSX_ARCHITECTURES=x86_64');
  if (platform === 'darwin-arm64') flags.push('-DCMAKE_OSX_ARCHITECTURES=arm64');
  if (platform.startsWith('darwin')) flags.push('-DCMAKE_OSX_DEPLOYMENT_TARGET=11.0');
  command('cmake', ['-S', source, '-B', build, ...flags], cache);
  command('cmake', ['--build', build, '--config', 'Release', '--target', 'whisper-cli', '--parallel', '2'], cache);
  const name = platform.startsWith('win32') ? 'whisper-cli.exe' : 'whisper-cli';
  const candidates = [path.join(build, 'bin', 'Release', name), path.join(build, 'bin', name)];
  const binary = (await Promise.all(candidates.map(async candidate => await exists(candidate) ? candidate : null))).find(Boolean);
  if (!binary) throw new Error('The pinned Whisper build did not produce whisper-cli');
  const dest = path.join(assets, 'whisper'); await fs.mkdir(dest, { recursive: true });
  const existing = await fs.readdir(dest);
  if (existing.some(item => ![name, 'LICENSE', 'source-provenance.json', '.gitkeep'].includes(item))) throw new Error('Whisper destination contains a different runtime. Use an empty asset directory.');
  await fs.copyFile(binary, path.join(dest, name));
  await fs.copyFile(path.join(source, 'LICENSE'), path.join(dest, 'LICENSE'));
  // Resource executables are not automatically signed by Tauri's app-bundle
  // signer. Intel's linker leaves this binary unsigned; ARM's incidental
  // linker signature is not a portable packaging contract. Sign before the
  // provenance inventory is sealed, so it describes the shipped bytes.
  if (platform.startsWith('darwin-')) {
    command('codesign', ['--force', '--sign', '-', '--timestamp=none', path.join(dest, name)], dest);
    command('codesign', ['--verify', '--strict', path.join(dest, name)], dest);
  }
  // --help is written to stderr by some official releases; capture both streams.
  const help = command(path.join(dest, name), ['--help'], dest, true);
  if (/deprecated/i.test(help) || !help.includes('--model') || !help.includes('--output-txt')) throw new Error('Refusing an incomplete or deprecated Whisper CLI');
  await fs.writeFile(path.join(dest, 'source-provenance.json'), JSON.stringify({ repository: pins.whisper.repository, commit: actual,
    target: targets[platform], flags, cmake: command('cmake', ['--version'], cache, true).split(/\r?\n/)[0], executable_sha256: await digest(path.join(dest, name)) }, null, 2) + '\n');
}
export async function run(options = {}) {
  const platform = platformFor(options.target);
  const pins = JSON.parse(await fs.readFile(pinPath, 'utf8'));
  const assets = path.resolve(options.assetsDir ?? path.join(root, 'assets'));
  const cache = path.resolve(options.workDir ?? path.join(root, 'target', 'desktop-assets', platform));
  const receiptPath = path.join(assets, 'desktop-provenance.json');
  const manifest = await digest(pinPath);
  if (options.verify) {
    const receipt = JSON.parse(await fs.readFile(receiptPath, 'utf8'));
    if (receipt.manifest_sha256 !== manifest || receipt.platform !== platform || !receipt.complete) throw new Error('Desktop asset provenance does not match the pinned platform/manifest');
    const actual = await resourceInventory(assets);
    if (JSON.stringify(actual) !== JSON.stringify(receipt.files)) throw new Error('Desktop runtime inventory changed since verified preparation');
    for (const pin of pins.files) await verifyFile(checkedPath(assets, pin.path), pin);
    if (platform.startsWith('darwin-')) {
      // Refuse a previously prepared unsigned runtime before a costly rebuild.
      command('codesign', ['--verify', '--strict', path.join(assets, 'whisper', 'whisper-cli')], assets);
    }
    console.log(`[desktop-assets] verified ${actual.length} files for ${platform}`); return;
  }
  if (await exists(receiptPath)) return run({ ...options, verify: true });
  await fs.mkdir(assets, { recursive: true }); await fs.mkdir(cache, { recursive: true });
  for (const pin of pins.files) await download(pin.url, checkedPath(assets, pin.path), pin);
  await preparePiper(pins, platform, assets, cache);
  if (options.dataOnly) { console.log('[desktop-assets] model/embedding/Piper data verified; Whisper build still required. No complete receipt written.'); return; }
  await prepareWhisper(pins, platform, assets, cache);
  const receipt = { version: 1, complete: true, platform, manifest_sha256: manifest, files: await resourceInventory(assets) };
  await fs.writeFile(receiptPath, JSON.stringify(receipt, null, 2) + '\n', { flag: 'wx' });
  await run({ ...options, verify: true });
}
if (process.argv[1] && import.meta.url === pathToFileURL(path.resolve(process.argv[1])).href) {
  const options = {};
  try {
    for (let i = 2; i < process.argv.length; i++) {
      const arg = process.argv[i];
      if (arg === '--verify') options.verify = true;
      else if (arg === '--data-only') options.dataOnly = true;
      else if (['--assets-dir', '--work-dir', '--target'].includes(arg)) {
        const value = process.argv[++i]; if (!value || value.startsWith('--')) throw new Error(`Missing ${arg} value`);
        options[{ '--assets-dir': 'assetsDir', '--work-dir': 'workDir', '--target': 'target' }[arg]] = value;
      } else throw new Error(`Unknown argument: ${arg}`);
    }
    await run(options);
  } catch (error) { console.error(`[desktop-assets] ${error.message}`); process.exitCode = 1; }
}
