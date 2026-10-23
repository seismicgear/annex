#!/usr/bin/env node
// Build and seal the private installer before Tauri seals its containing app.
// Tauri custom Contents files retain their signatures; they are not re-signed
// with the renderer application's sandbox/camera/microphone entitlements.
import { execFileSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
if (process.platform !== 'darwin') throw Error('The bundled installer must be built on macOS');
const target = process.env.TAURI_ENV_TARGET_TRIPLE ||
  (process.arch === 'arm64' ? 'aarch64-apple-darwin' : 'x86_64-apple-darwin');
if (!['aarch64-apple-darwin', 'x86_64-apple-darwin'].includes(target)) {
  throw Error('Unsupported macOS installer target');
}
const publicKey = process.env.TAURI_SIGNING_PUBLIC_KEY || '';
if (process.env.ANNEX_BUILD_PROFILE === 'production' &&
    process.env.ANNEX_UNSIGNED_DRY_RUN !== 'true' && !publicKey) {
  throw Error('The macOS installer requires the same verification key as the desktop updater');
}
// Neither the compiler nor a service binary ever receives the private updater
// signing key. The release CLI still receives it in its own parent process.
const compilerEnv = { ...process.env };
delete compilerEnv.TAURI_SIGNING_PRIVATE_KEY;
delete compilerEnv.TAURI_SIGNING_PRIVATE_KEY_PASSWORD;
execFileSync('cargo', ['build', '--locked', '--release', '--target', target,
  '-p', 'annex-macos-updater'], { cwd: root, stdio: 'inherit', env: compilerEnv });

const destination = path.join(root, 'assets/macos-installer/com.annex.desktop.installer.xpc');
const contents = path.join(destination, 'Contents');
fs.mkdirSync(path.join(contents, 'MacOS'), { recursive: true });
fs.copyFileSync(path.join(root, 'crates/annex-macos-updater/Info.plist'), path.join(contents, 'Info.plist'));
fs.copyFileSync(path.join(root, 'target', target, 'release/annex-macos-updater'),
  path.join(contents, 'MacOS/annex-macos-updater'));
fs.chmodSync(path.join(contents, 'MacOS/annex-macos-updater'), 0o755);

// Do not print command arguments or subprocess errors that can contain a
// keychain/import password. Only a temporary, explicitly selected keychain is
// touched; the user's default keychain and search list remain unchanged.
function security(args) {
  try {
    execFileSync('/usr/bin/security', args, { stdio: 'pipe', timeout: 60_000 });
  } catch {
    throw Error(`macOS signing preparation failed (${args[0]})`);
  }
}
const signed = process.env.ANNEX_NATIVE_SIGNING === 'true';
const identity = signed ? process.env.APPLE_SIGNING_IDENTITY : '-';
if (!identity || (signed && (!process.env.APPLE_CERTIFICATE ||
    !process.env.APPLE_CERTIFICATE_PASSWORD || !process.env.APPLE_TEAM_ID))) {
  throw Error('Developer ID installer signing configuration is incomplete');
}
let scratch, keychain, created = false;
try {
  if (signed) {
    scratch = fs.mkdtempSync(path.join(os.tmpdir(), 'annex-installer-signing-'));
    fs.chmodSync(scratch, 0o700);
    keychain = path.join(scratch, 'installer.keychain-db');
    const certificate = path.join(scratch, 'certificate.p12');
    const password = randomBytes(32).toString('hex');
    fs.writeFileSync(certificate, Buffer.from(process.env.APPLE_CERTIFICATE, 'base64'), { mode: 0o600 });
    security(['create-keychain', '-p', password, keychain]);
    created = true;
    security(['set-keychain-settings', '-lut', '3600', keychain]);
    security(['unlock-keychain', '-p', password, keychain]);
    security(['import', certificate, '-k', keychain, '-P', process.env.APPLE_CERTIFICATE_PASSWORD,
      '-T', '/usr/bin/codesign']);
    security(['set-key-partition-list', '-S', 'apple-tool:,apple:,codesign:', '-s', '-k', password, keychain]);
  }
  const args = ['--force', '--sign', identity, '--options', 'runtime',
    ...(signed ? ['--timestamp', '--keychain', keychain] : []), destination];
  execFileSync('/usr/bin/codesign', args, { stdio: 'inherit' });
  execFileSync('/usr/bin/codesign', ['--verify', '--strict', destination], { stdio: 'inherit' });
} finally {
  if (created) security(['delete-keychain', keychain]);
  if (scratch) fs.rmSync(scratch, { recursive: true });
}
console.log(`Built sealed macOS installation service (${target}, ${signed ? 'Developer ID' : 'ad-hoc'}).`);
