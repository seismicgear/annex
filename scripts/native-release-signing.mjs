#!/usr/bin/env node
// Native OS trust is independent of Tauri updater signatures.
import { pathToFileURL } from 'node:url';
import { parseArgs } from 'node:util';

function required(env, name) {
  const value = env[name]?.trim();
  if (!value) throw new Error(`Native signing requires ${name}`);
  return value;
}

export function signingEnabled(env) {
  const mode = required(env, 'ANNEX_NATIVE_SIGNING');
  if (!['true', 'false'].includes(mode)) throw new Error('Invalid ANNEX_NATIVE_SIGNING');
  if (mode === 'false' && !(env.GITHUB_EVENT_NAME === 'workflow_dispatch'
      && env.ANNEX_DRY_RUN === 'true' && !env.GITHUB_REF?.startsWith('refs/tags/'))) {
    throw new Error('OS-unsigned packages are allowed only in an explicit branch draft');
  }
  if (mode === 'true' && env.ANNEX_UNSIGNED_DRY_RUN === 'true') {
    throw new Error('Native release signing also requires updater signing');
  }
  return mode === 'true';
}

export function nativeConfig(platform, env, config = {}) {
  if (!['win32', 'darwin'].includes(platform)) throw new Error('Unsupported signing platform');
  const result = structuredClone(config);
  if (!signingEnabled(env)) return result;
  result.bundle ??= {};
  if (platform === 'win32') {
    required(env, 'WINDOWS_CERTIFICATE');
    required(env, 'WINDOWS_CERTIFICATE_PASSWORD');
    const thumbprint = required(env, 'WINDOWS_CERTIFICATE_THUMBPRINT');
    if (!/^[A-Fa-f0-9]{40}$/.test(thumbprint)) throw new Error('Invalid Windows publisher thumbprint');
    const timestamp = new URL(required(env, 'WINDOWS_TIMESTAMP_URL'));
    if (timestamp.protocol !== 'https:' || timestamp.username || timestamp.password || timestamp.hash) {
      throw new Error('Windows timestamp service must use HTTPS without credentials or a fragment');
    }
    result.bundle.windows = {
      ...result.bundle.windows, certificateThumbprint: thumbprint.toUpperCase(),
      digestAlgorithm: 'sha256', timestampUrl: timestamp.href, tsp: true,
    };
  } else {
    for (const name of ['APPLE_CERTIFICATE', 'APPLE_CERTIFICATE_PASSWORD', 'APPLE_ID', 'APPLE_PASSWORD']) {
      required(env, name);
    }
    const team = required(env, 'APPLE_TEAM_ID');
    const identity = required(env, 'APPLE_SIGNING_IDENTITY');
    if (!/^[A-Z0-9]{10}$/.test(team)
        || !identity.startsWith('Developer ID Application: ')
        || !identity.endsWith(` (${team})`) || /[\r\n]/.test(identity)) {
      throw new Error('A Developer ID Application identity for the approved Apple team is required');
    }
    result.bundle.macOS = { ...result.bundle.macOS, signingIdentity: identity, hardenedRuntime: true };
  }
  return result;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  try {
    const { values } = parseArgs({ options: {
      preflight: { type: 'boolean' }, platform: { type: 'string' }, config: { type: 'string', default: '{}' },
    } });
    if (values.preflight) {
      nativeConfig('win32', process.env);
      nativeConfig('darwin', process.env);
      console.log(signingEnabled(process.env)
        ? 'Native publisher signing and notarization credentials are configured; verification remains mandatory.'
        : 'Explicit OS-unsigned branch draft; production publisher trust is not qualified.');
    } else {
      console.log(JSON.stringify(nativeConfig(values.platform, process.env, JSON.parse(values.config))));
    }
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
