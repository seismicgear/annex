import test from 'node:test';
import assert from 'node:assert/strict';
import { machoArchitecture, dylibDependencies, validateDependencyClosure, validateWave, buildMacPiper } from '../build-macos-piper.mjs';

test('Mach-O architecture validation detects Intel mislabeled as ARM and rejects universal/non-native files', () => {
  const executable = Buffer.alloc(32);
  executable.writeUInt32LE(0xfeedfacf, 0);
  executable.writeUInt32LE(0x01000007, 4);
  assert.equal(machoArchitecture(executable), 'x86_64');
  assert.notEqual(machoArchitecture(executable), 'arm64');
  executable.writeUInt32LE(0x0100000c, 4);
  assert.equal(machoArchitecture(executable), 'arm64');
  executable.writeUInt32BE(0xcafebabe, 0);
  assert.throws(() => machoArchitecture(executable), /thin native/);
  assert.throws(() => machoArchitecture(Buffer.alloc(2)), /thin native/);
});

test('runtime closure requires packaged loader-relative dependencies and permits only Apple system libraries', () => {
  const libraries = new Set(['libespeak-ng.1.dylib', 'libonnxruntime.1.14.1.dylib']);
  const dependencies = dylibDependencies('/a path/piper:\n\t@loader_path/libespeak-ng.1.dylib (compatibility version 1.0.0, current version 1.0.0)\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0, current version 1336.61.1)\n');
  validateDependencyClosure(dependencies, libraries);
  for (const dependency of ['@rpath/libespeak-ng.1.dylib', '@loader_path/missing.dylib', '@loader_path/../libespeak-ng.1.dylib', '/usr/local/lib/libespeak-ng.1.dylib', '/opt/homebrew/lib/libespeak-ng.1.dylib']) {
    assert.throws(() => validateDependencyClosure([dependency], libraries), /Unbundled or non-relative/);
  }
  assert.throws(() => dylibDependencies('piper:\ninvalid output\n'), /Unrecognized/);
});

test('speech execution proof requires complete WAV data with actual nonzero samples', () => {
  const wav = Buffer.alloc(2044);
  wav.write('RIFF', 0); wav.writeUInt32LE(wav.length - 8, 4); wav.write('WAVE', 8);
  wav.write('fmt ', 12); wav.writeUInt32LE(16, 16);
  wav.write('data', 36); wav.writeUInt32LE(2000, 40);
  assert.throws(() => validateWave(wav), /no audible samples/);
  wav[1000] = 1; validateWave(wav);
  assert.throws(() => validateWave(wav.subarray(0, 1000)), /Truncated/);
  assert.throws(() => validateWave(Buffer.from('not audio')), /did not produce/);
});

test('native source builder refuses another OS or architecture before touching disk', async () => {
  const platform = process.platform === 'darwin' && process.arch === 'arm64' ? 'darwin-x64' : 'darwin-arm64';
  await assert.rejects(buildMacPiper({ platform }), /matching native macOS runner/);
});
