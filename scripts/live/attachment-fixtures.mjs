// Public component fixtures generated from actual registered identities and
// genuine proofs. The isolated authority must use the publicly known test seed
// [0x55;32]; this must never be a production server or production identity key.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, realpathSync } from 'node:fs';
import { createPublicKey, verify, randomBytes } from 'node:crypto';
import { dirname, resolve, relative, isAbsolute } from 'node:path';
import { fileURLToPath } from 'node:url';
import { register, request, digest, signingIdentityFromSeed } from './protocol.mjs';
import { makeDeviceCertificate, certificateDigest } from './device-certificates.mjs';

const canonical = value => Array.isArray(value) ? value.map(canonical)
  : value && typeof value === 'object'
    ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;

function eventFor(member, authority, channel, seq, previous, operation) {
  const certificate = member.certificate;
  const event = {
    version: 1, authority_identity: authority, channel_id: channel, seq,
    previous_hash: previous, operation, signer_pseudonym_id: certificate.pseudonym_id,
    signer_device_id: certificate.device_id,
    signer_certificate_sha256: certificateDigest(certificate),
  };
  const input = JSON.stringify(['annex-channel-manifest-v1', event.version, authority,
    channel, seq, previous, canonical(operation), event.signer_pseudonym_id,
    event.signer_device_id, event.signer_certificate_sha256]);
  event.event_hash = digest(input);
  event.signature_hex = member.signing.sign(input);
  const key = createPublicKey({ key: Buffer.concat([
    Buffer.from('302a300506032b6570032100', 'hex'),
    Buffer.from(certificate.ed25519_pub_hex, 'hex'),
  ]), format: 'der', type: 'spki' });
  assert(verify(null, Buffer.from(input), key, Buffer.from(event.signature_hex, 'hex')));
  return event;
}

async function main(session, outputPath, privateOutputPath = null) {
  assert(session.binary_sha256 && session.url, 'Pass an actual isolated session manifest');
  if (privateOutputPath) {
    const repoRoot = realpathSync(fileURLToPath(new URL('../../', import.meta.url)));
    const privateParent = realpathSync(dirname(resolve(privateOutputPath)));
    const inside = relative(repoRoot, privateParent);
    assert(inside.startsWith('..') || isAbsolute(inside), 'Private fixture material must remain outside the repository');
    const workRoot = realpathSync(fileURLToPath(new URL('../../../', import.meta.url)));
    const workRelative = relative(workRoot, privateParent);
    assert(!workRelative.startsWith('..') && !isAbsolute(workRelative), 'Private fixtures must remain in this task work directory');
  }
  const testKey = signingIdentityFromSeed('55'.repeat(32));
  const expectedAuthority = `annex-server:${digest(Buffer.from(testKey.publicKey, 'hex'))}`;
  const members = [];
  for (let index = 0; index < 2; index++) {
    const identity = await register(session.url);
    const context = await request(session.url, '/api/keys/certificate-context', { token: identity.token });
    assert.equal(context.server_identity, expectedAuthority, 'Fixture generation requires the known isolated test authority');
    assert.equal(context.server_ed25519_pub_hex, testKey.publicKey);
    const signingSeed = randomBytes(32).toString('hex');
    const member = await makeDeviceCertificate(identity, context, { signingSeed });
    member.identity = identity;
    member.signingSeed = signingSeed;
    // Actual admission verifies the genuine proof before a fixture is retained.
    const response = await request(session.url, '/api/keys/me', {
      method: 'PUT', token: identity.token, body: {
        version: 2, x25519_pub_hex: member.certificate.x25519_pub_hex,
        device_certificate: member.certificate,
      },
    });
    assert.equal(response.created, true);
    members.push(member);
  }
  const channels = {};
  for (const channel of ['general', 'room']) {
    const genesis = eventFor(members[0], expectedAuthority, channel, 1, '', {
      kind: 'genesis', join_policy: 'eligible_self_join', history_policy: 'all_members',
      certificate: members[0].certificate,
    });
    const join = eventFor(members[1], expectedAuthority, channel, 2, genesis.event_hash, {
      kind: 'join', certificate: members[1].certificate,
    });
    channels[channel] = [genesis, join];
  }
  const vkey = JSON.parse(readFileSync(new URL('../../zk/keys/membership_v2_vkey.json', import.meta.url), 'utf8'));
  writeFileSync(outputPath, JSON.stringify({
    version: 1, authority_identity: expectedAuthority,
    authority_public_key: testKey.publicKey,
    certificates: members.map(member => member.certificate), channels, vkey,
    provenance: {
      created: new Date().toISOString(), binary_sha256: session.binary_sha256,
      kind: 'Actual registered membership proofs and certificate API admissions; independently signed public manifests for component tests. Manifest API acceptance is separate.',
    },
  }, null, 2));
  if (privateOutputPath) {
    writeFileSync(privateOutputPath, JSON.stringify({
      version: 1, scope: 'Disposable isolated acceptance identities only; never commit or include in receipts',
      authority_test_seed: '55'.repeat(32),
      members: members.map(member => ({
        identity: { ...member.identity, sk: String(member.identity.sk) },
        certificate: member.certificate,
        ed25519_seed_hex: member.signingSeed,
        x25519_pkcs8_base64: member.device.privateKey.export({ format: 'der', type: 'pkcs8' }).toString('base64'),
      })),
    }, null, 2), { flag: 'wx', mode: 0o600 });
  }
  console.log(`Public attachment fixtures created from two genuine certificate admissions. ${privateOutputPath ? 'Disposable private material saved only to the explicitly selected task work file.' : 'No private material retained.'}`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [sessionPath, outputPath, privateOutputPath] = process.argv.slice(2);
  assert(sessionPath && outputPath, 'Supply isolated session JSON and public fixture output path');
  const session = JSON.parse(readFileSync(sessionPath, 'utf8').replace(/^\uFEFF/, ''));
  try {
    await main(session, outputPath, privateOutputPath);
    process.exit(0);
  } catch (error) {
    console.error(error.message);
    process.exit(1);
  }
}
