// Create a NEW member-signed lineage at a distinct controlled authority, using
// previously genuine issuer55 certificates. Private disposable signer material
// is read only from this task's work directory, never copied into the fixture.
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, realpathSync } from 'node:fs';
import { createPublicKey, verify } from 'node:crypto';
import { relative, isAbsolute, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { signingIdentityFromSeed, digest } from './protocol.mjs';
import { certificateDigest } from './device-certificates.mjs';

const [privateFile, outputFile] = process.argv.slice(2);
assert(privateFile && outputFile, 'Supply disposable private work fixture and public output');
const repo = realpathSync(fileURLToPath(new URL('../../', import.meta.url)));
const work = realpathSync(fileURLToPath(new URL('../../../', import.meta.url)));
const secret = realpathSync(privateFile);
const inWork = relative(work, secret); const outsideRepo = relative(repo, secret);
assert(!inWork.startsWith('..') && !isAbsolute(inWork));
assert(outsideRepo.startsWith('..') || isAbsolute(outsideRepo));
assert(resolve(outputFile) !== secret);
const controlled = JSON.parse(readFileSync(secret, 'utf8'));
const original = JSON.parse(readFileSync(new URL('../../crates/annex-server/tests/fixtures/attachment-certified-public.json', import.meta.url), 'utf8'));
const authority = signingIdentityFromSeed('66'.repeat(32));
const authorityId = `annex-server:${digest(Buffer.from(authority.publicKey, 'hex'))}`;
const canonical = value => Array.isArray(value) ? value.map(canonical) : value && typeof value === 'object'
  ? Object.fromEntries(Object.keys(value).sort().map(key => [key, canonical(value[key])])) : value;
let previous = '';
const events = controlled.members.map((member, index) => {
  assert.deepEqual(member.certificate, original.certificates[index]);
  const certificate = member.certificate;
  const signing = signingIdentityFromSeed(member.ed25519_seed_hex);
  assert.equal(signing.publicKey, certificate.ed25519_pub_hex);
  const operation = index === 0 ? { kind: 'genesis', join_policy: 'eligible_self_join', history_policy: 'all_members', certificate }
    : { kind: 'join', certificate };
  const event = { version: 1, authority_identity: authorityId, channel_id: 'general', seq: index + 1,
    previous_hash: previous, operation, signer_pseudonym_id: certificate.pseudonym_id,
    signer_device_id: certificate.device_id, signer_certificate_sha256: certificateDigest(certificate) };
  const bytes = JSON.stringify(['annex-channel-manifest-v1', event.version, authorityId, event.channel_id, event.seq,
    previous, canonical(operation), event.signer_pseudonym_id, event.signer_device_id, event.signer_certificate_sha256]);
  event.event_hash = digest(bytes); event.signature_hex = signing.sign(bytes); previous = event.event_hash;
  const key = createPublicKey({ key: Buffer.concat([Buffer.from('302a300506032b6570032100', 'hex'), Buffer.from(signing.publicKey, 'hex')]), format: 'der', type: 'spki' });
  assert(verify(null, Buffer.from(bytes), key, Buffer.from(event.signature_hex, 'hex')));
  return event;
});
writeFileSync(outputFile, JSON.stringify({ version: 1, authority_identity: authorityId, authority_public_key: authority.publicKey,
  channels: { general: events }, provenance: { source_certificate_fixture: 'attachment-certified-public.json',
    kind: 'New independently verified member signatures at distinct controlled authority66; genuine issuer55 device certificates unchanged. Component fixture, not API admission evidence.' } }, null, 2));
console.log('Wrote public remote-reader lineage; private signer material was not emitted.');
