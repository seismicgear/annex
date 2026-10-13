// @vitest-environment node
import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { certificateChallenge, certificateDigest, membershipVerifierFingerprint, parseCertificateContext,
  parseDeviceCertificate, verifyDeviceCertificate } from './device-certificate';

// Node has no browser Worker. Exercise the exact worker computation, with real
// pinned Groth16 verification; the transport lifecycle is tested separately.
vi.mock('./membership-verification', () => import('./membership-verification-core'));

// Public fixture from an actual registered identity and genuine Groth16 proof.
// No identity, X25519, Ed25519 or server private key is retained here.
const fixture = JSON.parse(readFileSync(new URL('./__fixtures__/device-certificate-public.json', import.meta.url), 'utf8'));
const certificate = parseDeviceCertificate(fixture.certificate);
const expected = { serverIdentity: certificate.server_identity, pseudonym: certificate.pseudonym_id,
  deviceId: certificate.device_id, publicKeyHex: certificate.x25519_pub_hex };

describe('independently pinned device certificate verification', () => {
  it('matches the Rust semantic verifier fingerprint and verifies an actual possession proof', async () => {
    expect(membershipVerifierFingerprint()).toBe('ef03c983f985ac5c9e6a35d536fab2453cd70319b1c5b4b2b2666be3450f24f2');
    expect(parseCertificateContext(fixture.context)).toEqual(fixture.context);
    expect(await verifyDeviceCertificate(certificate, expected)).toEqual(certificate);
  }, 30000);
  it('refuses a different server key or remotely advertised verifier', () => {
    expect(() => parseCertificateContext({ ...fixture.context, server_ed25519_pub_hex: '01'.repeat(32) })).toThrow('does not match');
    expect(() => parseCertificateContext({ ...fixture.context, verifier_sha256: 'ff'.repeat(32) })).toThrow('does not match');
  });
  it.each(['serverIdentity', 'pseudonym', 'deviceId', 'publicKeyHex'] as const)('binds %s to the independently expected context', async field => {
    await expect(verifyDeviceCertificate(certificate, { ...expected, [field]: '00'.repeat(32) })).rejects.toThrow('another identity');
  });
  it('refuses a validly shaped but false Groth16 proof, independently of the possession signature', async () => {
    const changed = structuredClone(certificate);
    changed.proof.pi_c = [...changed.proof.pi_a];
    await expect(verifyDeviceCertificate(changed, expected)).rejects.toThrow('membership proof is invalid');
  }, 30000);
  it('refuses a false Ed25519 possession signature before accepting proof ownership', async () => {
    await expect(verifyDeviceCertificate({ ...certificate, signature_hex: '00'.repeat(64) }, expected)).rejects.toThrow('possession signature');
  });
  it('binds every public signal to the certificate body rather than accepting any membership proof', async () => {
    const changed = structuredClone(certificate);
    changed.public_signals[4] = '00'.repeat(32);
    await expect(verifyDeviceCertificate(changed, expected)).rejects.toThrow('every certificate field');
  });
  it('requires canonical field, proof point, signature and lineage representations', () => {
    expect(() => parseDeviceCertificate({ ...certificate, root_hex: 'ff'.repeat(32) })).toThrow('outside the field');
    expect(() => parseDeviceCertificate({ ...certificate, generation: 2 })).toThrow('lineage');
    expect(() => parseDeviceCertificate({ ...certificate, signature_hex: certificate.signature_hex.toUpperCase() })).toThrow('signature');
    expect(() => parseDeviceCertificate({ ...certificate, proof: { ...certificate.proof, pi_a: ['0', '0', '0'] } })).toThrow('G1');
    expect(() => parseDeviceCertificate({ ...certificate, public_signals: [...certificate.public_signals, '00'.repeat(32)] })).toThrow('five');
  });
  it('keeps the certificate digest stable across proof randomness but binds replacement lineage', () => {
    expect(certificateDigest({ ...certificate, proof: {} } as typeof certificate)).toBe(certificateDigest(certificate));
    expect(certificateChallenge(certificate)).toBe(certificate.public_signals[4]);
    expect(certificateDigest({ ...certificate, generation: 2, previous_certificate_sha256: certificateDigest(certificate) })).not.toBe(certificateDigest(certificate));
  });
});
