// The upstream 2023 Mac archives omit dylibs and both contain Intel executables.
// Build the pinned sources on each native runner; do not relabel an Intel binary.
import fs from 'node:fs/promises';
import path from 'node:path';
import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';

const cpuTypes = new Map([[0x01000007, 'x86_64'], [0x0100000c, 'arm64']]);
export function machoArchitecture(bytes) {
  if (bytes.length < 32 || bytes.readUInt32LE(0) !== 0xfeedfacf) {
    throw new Error('Expected a thin native 64-bit Mach-O executable or library');
  }
  const architecture = cpuTypes.get(bytes.readUInt32LE(4));
  if (!architecture) throw new Error('Unsupported Mach-O architecture');
  return architecture;
}
export function dylibDependencies(output) {
  return output.split(/\r?\n/).slice(1).filter(line => line.trim()).map(line => {
    const match = /^\s+(.+?) \(compatibility version /.exec(line);
    if (!match) throw new Error(`Unrecognized otool dependency: ${line}`);
    return match[1];
  });
}
export function systemLibrary(dependency) {
  return dependency.startsWith('/usr/lib/') || dependency.startsWith('/System/Library/');
}
export function validateDependencyClosure(dependencies, filenames) {
  for (const dependency of dependencies) {
    if (systemLibrary(dependency)) continue;
    const name = dependency.slice('@loader_path/'.length);
    if (!dependency.startsWith('@loader_path/') || name.includes('/') || !filenames.has(name)) {
      throw new Error(`Unbundled or non-relative macOS runtime dependency: ${dependency}`);
    }
  }
}
export function validateWave(bytes) {
  if (bytes.length < 44 || bytes.toString('ascii', 0, 4) !== 'RIFF' || bytes.toString('ascii', 8, 12) !== 'WAVE') {
    throw new Error('Piper smoke synthesis did not produce a WAV file');
  }
  let pcm;
  for (let offset = 12; offset + 8 <= bytes.length;) {
    const length = bytes.readUInt32LE(offset + 4);
    if (offset + 8 + length > bytes.length) throw new Error('Truncated Piper smoke WAV');
    if (bytes.toString('ascii', offset, offset + 4) === 'data') pcm = bytes.subarray(offset + 8, offset + 8 + length);
    offset += 8 + length + (length % 2);
  }
  if (!pcm || pcm.length < 2000 || !pcm.some(value => value !== 0)) throw new Error('Piper smoke synthesis produced no audible samples');
}
function execute(binary, args, cwd, input) {
  const result = spawnSync(binary, args, { cwd, encoding: 'utf8', input,
    stdio: input === undefined ? 'inherit' : ['pipe', 'pipe', 'pipe'], timeout: 20 * 60_000, maxBuffer: 16 * 1024 * 1024 });
  if (result.error) throw result.error;
  if (result.status !== 0) throw new Error(`${binary} failed (${result.status}): ${result.stderr ?? ''}`);
}
export async function buildMacPiper({ pins, platform, assets, cache, download, command, inventory, checkedPath }) {
  if (process.platform !== 'darwin' || platform !== `darwin-${process.arch}`) throw new Error('Mac Piper requires the matching native macOS runner');
  const architecture = platform === 'darwin-arm64' ? 'arm64' : 'x86_64';
  const workspace = await fs.mkdtemp(path.join(cache, 'piper-native-'));
  const sources = {};
  const sourcePins = { ...pins.piper.macos_sources, onnxruntime: pins.piper.macos_onnxruntime[platform] };
  for (const [name, pin] of Object.entries(sourcePins)) {
    const archive = path.join(cache, pin.name);
    await download(pin.url, archive, pin);
    const members = command('tar', ['-tf', archive], cache, true).split(/\r?\n/).filter(Boolean);
    for (const member of members) {
      checkedPath(workspace, member.replace(/\/$/, ''));
      if (member !== `${pin.root}/` && !member.startsWith(`${pin.root}/`)) throw new Error(`Unexpected ${name} archive root`);
    }
    command('tar', ['-xf', archive, '-C', workspace], cache);
    sources[name] = path.join(workspace, pin.root);
    await inventory(sources[name]); // Also refuses links escaping this source archive.
  }
  const common = ['-DCMAKE_BUILD_TYPE=Release', '-DCMAKE_POLICY_VERSION_MINIMUM=3.5',
    `-DCMAKE_OSX_ARCHITECTURES=${architecture}`, '-DCMAKE_OSX_DEPLOYMENT_TARGET=11.0',
    '-DCMAKE_INSTALL_LIBDIR=lib', '-DCMAKE_POSITION_INDEPENDENT_CODE=ON',
    '-DCMAKE_INSTALL_RPATH=@loader_path', '-DCMAKE_BUILD_WITH_INSTALL_RPATH=OFF',
    '-DCMAKE_EXE_LINKER_FLAGS=-Wl,-headerpad_max_install_names',
    '-DCMAKE_SHARED_LINKER_FLAGS=-Wl,-headerpad_max_install_names',
    '-DFETCHCONTENT_FULLY_DISCONNECTED=ON'];
  const installed = {};
  function build(name, flags, targets) {
    const buildDir = path.join(workspace, `${name}-build`);
    const prefix = path.join(workspace, `${name}-installed`);
    command('cmake', ['-S', sources[name], '-B', buildDir, ...common, `-DCMAKE_INSTALL_PREFIX=${prefix}`, ...flags], workspace);
    command('cmake', ['--build', buildDir, '--config', 'Release', '--parallel', '2', '--target', ...targets], workspace);
    command('cmake', ['--install', buildDir, '--config', 'Release'], workspace);
    installed[name] = prefix;
  }
  build('fmt', ['-DBUILD_SHARED_LIBS=OFF', '-DFMT_TEST=OFF', '-DFMT_DOC=OFF'], ['fmt']);
  build('spdlog', ['-DSPDLOG_BUILD_SHARED=OFF', '-DSPDLOG_BUILD_TESTS=OFF', '-DSPDLOG_BUILD_EXAMPLE=OFF',
    '-DSPDLOG_FMT_EXTERNAL=ON', `-Dfmt_DIR=${installed.fmt}/lib/cmake/fmt`], ['spdlog']);
  build('espeak', ['-DBUILD_SHARED_LIBS=ON', '-DBUILD_TESTING=OFF', '-DESPEAK_BUILD_MANPAGES=OFF',
    '-DUSE_ASYNC=OFF', '-DUSE_MBROLA=OFF', '-DUSE_LIBSONIC=OFF', '-DUSE_LIBPCAUDIO=OFF',
    '-DUSE_KLATT=OFF', '-DUSE_SPEECHPLAYER=OFF', '-DEXTRA_cmn=ON', '-DEXTRA_ru=ON',
    '-DCMAKE_C_FLAGS=-D_FILE_OFFSET_BITS=64', `-DFETCHCONTENT_SOURCE_DIR_SONIC-GIT=${sources.sonic}`], ['espeak-ng-bin', 'data']);
  build('phonemize', [`-DESPEAK_NG_DIR=${installed.espeak}`, `-DONNXRUNTIME_DIR=${sources.onnxruntime}`], ['piper_phonemize', 'piper_phonemize_exe']);
  build('piper', [`-DFMT_DIR=${installed.fmt}`, `-DSPDLOG_DIR=${installed.spdlog}`, `-DPIPER_PHONEMIZE_DIR=${installed.phonemize}`,
    '-DCMAKE_CXX_FLAGS=-DSPDLOG_FMT_EXTERNAL'], ['piper']);
  const stage = path.join(workspace, 'bundle');
  await fs.mkdir(stage);
  // Copy dylibs explicitly: upstream Piper's install rule only matches DLL/SO.
  for (const name of ['piper', 'piper_phonemize', 'espeak-ng']) {
    await fs.copyFile(path.join(installed.piper, name), path.join(stage, name));
    await fs.chmod(path.join(stage, name), 0o755);
  }
  const libDir = path.join(installed.phonemize, 'lib');
  for (const name of await fs.readdir(libDir)) {
    if (name.endsWith('.dylib')) await fs.copyFile(path.join(libDir, name), path.join(stage, name));
  }
  await fs.cp(path.join(installed.phonemize, 'share', 'espeak-ng-data'), path.join(stage, 'espeak-ng-data'), { recursive: true, dereference: true });
  await fs.copyFile(path.join(installed.phonemize, 'share', 'libtashkeel_model.ort'), path.join(stage, 'libtashkeel_model.ort'));
  const files = new Set(await fs.readdir(stage));
  const native = [...files].filter(name => name.endsWith('.dylib') || ['piper', 'piper_phonemize', 'espeak-ng'].includes(name));
  if (!native.some(name => name.startsWith('libespeak-ng.')) || !native.some(name => name.startsWith('libonnxruntime.')) || !native.some(name => name.startsWith('libpiper_phonemize.'))) throw new Error('Missing required Piper dylibs');
  for (const name of native) {
    const filename = path.join(stage, name);
    if (machoArchitecture(await fs.readFile(filename)) !== architecture) throw new Error(`Wrong architecture for ${name}; refusing Rosetta packaging`);
    const dependencies = dylibDependencies(command('otool', ['-L', filename], workspace, true));
    for (const dependency of dependencies) {
      if (systemLibrary(dependency)) continue;
      const dependencyName = path.posix.basename(dependency);
      if (!files.has(dependencyName)) throw new Error(`Cannot bundle dependency ${dependency} for ${name}`);
      // The first entry of a dylib is its install ID, not a loaded dependency.
      if (name.endsWith('.dylib') && dependency === dependencies[0]) continue;
      command('install_name_tool', ['-change', dependency, `@loader_path/${dependencyName}`, filename], workspace);
    }
    if (name.endsWith('.dylib')) command('install_name_tool', ['-id', `@loader_path/${name}`, filename], workspace);
    validateDependencyClosure(dylibDependencies(command('otool', ['-L', filename], workspace, true)), files);
    // Changing load commands invalidates a previous ad-hoc ARM signature.
    command('codesign', ['--force', '--sign', '-', '--timestamp=none', filename], workspace);
    command('codesign', ['--verify', '--strict', filename], workspace);
  }
  const licenses = path.join(stage, 'licenses'); await fs.mkdir(licenses);
  for (const [name, source] of Object.entries(sources)) {
    for (const entry of await fs.readdir(source, { withFileTypes: true })) {
      if (entry.isFile() && /^(licen[cs]e|copying|thirdpartynotices)/i.test(entry.name)) {
        await fs.copyFile(path.join(source, entry.name), path.join(licenses, `${name}-${entry.name}`));
      }
    }
  }
  const destination = path.join(assets, 'piper');
  await fs.mkdir(destination, { recursive: true });
  if ((await inventory(destination)).length) throw new Error('Piper destination contains a different runtime. Use an empty asset directory.');
  await fs.cp(stage, destination, { recursive: true });
  const binary = path.join(destination, 'piper');
  const help = command(binary, ['--help'], destination, true);
  if (!help.includes('--model') || !help.includes('--output_raw')) throw new Error('Piper runtime is not the expected CLI');
  const wav = path.join(workspace, 'piper-native-smoke.wav');
  execute(binary, ['--model', path.join(assets, 'voices', 'en_US-lessac-medium.onnx'), '--output_file', wav], destination,
    'Annex native speech packaging verification.\n');
  const audio = await fs.readFile(wav); validateWave(audio);
  await fs.writeFile(path.join(destination, 'source-provenance.json'), JSON.stringify({
    platform, architecture, sources: sourcePins, cmake_flags: common, native_files: native,
    runtime_dependency_closure: 'bundled loader-relative and Apple system libraries only',
    smoke: { generated_wave_bytes: audio.length, sha256: createHash('sha256').update(audio).digest('hex') },
  }, null, 2) + '\n', { flag: 'wx' });
}
