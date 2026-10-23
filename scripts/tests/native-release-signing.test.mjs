import test from 'node:test';
import assert from 'node:assert/strict';
import { nativeConfig, signingEnabled } from '../native-release-signing.mjs';

const draft = { ANNEX_NATIVE_SIGNING: 'false', GITHUB_EVENT_NAME: 'workflow_dispatch',
  ANNEX_DRY_RUN: 'true', GITHUB_REF: 'refs/heads/candidate' };
const signed = { ...draft, ANNEX_NATIVE_SIGNING: 'true',
  WINDOWS_CERTIFICATE: 'opaque-pfx-fixture', WINDOWS_CERTIFICATE_PASSWORD: 'fixture-password',
  WINDOWS_CERTIFICATE_THUMBPRINT: 'ab'.repeat(20), WINDOWS_TIMESTAMP_URL: 'https://timestamp.example.test/',
  APPLE_CERTIFICATE: 'opaque-p12-fixture', APPLE_CERTIFICATE_PASSWORD: 'fixture-password',
  APPLE_ID: 'fixture@example.test', APPLE_PASSWORD: 'fixture-app-password', APPLE_TEAM_ID: 'AB12345678',
  APPLE_SIGNING_IDENTITY: 'Developer ID Application: Test Publisher (AB12345678)' };

test('OS-unsigned output is confined to an explicit branch draft', () => {
  assert.equal(signingEnabled(draft), false);
  for (const change of [{ GITHUB_REF: 'refs/tags/v0.1.3' }, { GITHUB_EVENT_NAME: 'push' },
    { ANNEX_DRY_RUN: 'false' }, { ANNEX_NATIVE_SIGNING: 'unexpected' }, { ANNEX_NATIVE_SIGNING: '' }]) {
    assert.throws(() => signingEnabled({ ...draft, ...change }));
  }
  assert.throws(() => signingEnabled({ ...signed, ANNEX_UNSIGNED_DRY_RUN: 'true' }));
});
test('draft keeps updater and unrelated configuration intact', () => {
  const config = { bundle: { createUpdaterArtifacts: true }, plugins: { updater: { pubkey: 'public' } } };
  assert.deepEqual(nativeConfig('darwin', draft, config), config);
  assert.deepEqual(nativeConfig('win32', draft, config), config);
});
test('Windows config binds SHA256, RFC3161 timestamp and exact publisher without exporting secrets', () => {
  const config = nativeConfig('win32', signed, { bundle: { createUpdaterArtifacts: true } });
  assert.equal(config.bundle.windows.certificateThumbprint, 'AB'.repeat(20));
  assert.equal(config.bundle.windows.digestAlgorithm, 'sha256');
  assert.equal(config.bundle.windows.tsp, true);
  assert.equal(config.bundle.createUpdaterArtifacts, true);
  assert(!JSON.stringify(config).includes('fixture-password'));
  for (const change of [{ WINDOWS_CERTIFICATE: '' }, { WINDOWS_CERTIFICATE_THUMBPRINT: 'arbitrary' },
    { WINDOWS_TIMESTAMP_URL: 'http://timestamp.example.test' }, { WINDOWS_TIMESTAMP_URL: 'https://secret@timestamp.example.test' }]) {
    assert.throws(() => nativeConfig('win32', { ...signed, ...change }));
  }
});
test('macOS requires notarization credentials and approved Developer ID team', () => {
  const config = nativeConfig('darwin', signed, { bundle: { macOS: { signingIdentity: '-', entitlements: 'Entitlements.plist' } } });
  assert.equal(config.bundle.macOS.signingIdentity, signed.APPLE_SIGNING_IDENTITY);
  assert.equal(config.bundle.macOS.hardenedRuntime, true);
  assert.equal(config.bundle.macOS.entitlements, 'Entitlements.plist');
  assert(!JSON.stringify(config).includes(signed.APPLE_PASSWORD));
  for (const change of [{ APPLE_PASSWORD: '' }, { APPLE_CERTIFICATE: '' },
    { APPLE_TEAM_ID: 'XY12345678' }, { APPLE_SIGNING_IDENTITY: '-' },
    { APPLE_SIGNING_IDENTITY: 'Apple Development: Test (AB12345678)' }]) {
    assert.throws(() => nativeConfig('darwin', { ...signed, ...change }));
  }
});
