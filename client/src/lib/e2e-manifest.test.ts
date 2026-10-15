// @vitest-environment node
import { readFileSync } from 'node:fs';
import { beforeAll, describe, expect, it, vi } from 'vitest';
import { certificateDigest } from './device-certificate';
import { applyManifestEvent, checkManifestPin, checkManifestRecipient, emptyManifest, manifestProjection,
  parseManifestEvent, verifyWrapAuthorization, type VerifiedManifest } from './e2e-manifest';

// Real pinned proof computation runs directly in this Node test environment.
vi.mock('./membership-verification', () => import('./membership-verification-core'));

// Genuine independently generated registry proofs, Ed25519 events, and Node
// X25519 sealed wraps. This file contains no signing/decryption secrets.
const fixture = JSON.parse(readFileSync(new URL('./__fixtures__/signed-manifest-public.json', import.meta.url), 'utf8'));
const states = new Map<number, VerifiedManifest>();
beforeAll(async () => {
  let state = emptyManifest(fixture.authority_identity, fixture.channel_id);
  states.set(0, state);
  for (const event of fixture.events) { state = await applyManifestEvent(state, event); states.set(event.seq, state); }
}, 30000);

describe('member-signed channel authorization', () => {
  it('independently verifies two actual ownership proofs and reconstructs all nine signed transitions', () => {
    expect(states.get(1)?.members.size).toBe(1);
    expect(states.get(2)?.members.size).toBe(2);
    expect(states.get(4)?.members.size).toBe(1);
    expect(states.get(5)?.members.get(fixture.certificates[0].pseudonym_id)?.role).toBe('member');
    expect(states.get(6)?.joinPolicy).toBe('moderator_admission');
    expect(states.get(8)?.members.size).toBe(2);
    expect(states.get(9)?.revokedDevices.has(fixture.certificates[0].device_id)).toBe(true);
    expect(manifestProjection(states.get(9)!).members.flatMap(member => member.devices)).toHaveLength(1);
  });
  it('refuses a cryptographically valid nonmoderator removal', async () => {
    await expect(applyManifestEvent(states.get(2)!, fixture.invalid_events.non_moderator_removal)).rejects.toThrow('moderator');
    expect(states.get(2)?.members.size).toBe(2);
  });
  it('refuses a valid outsider ownership proof and signature as private-channel admission', async () => {
    await expect(applyManifestEvent(states.get(7)!, fixture.invalid_events.private_self_join)).rejects.toThrow('self-admission');
    expect(states.get(7)?.members.size).toBe(1);
  });
  it('refuses a valid identity proof that attempts to reactivate the revoked device', async () => {
    await expect(applyManifestEvent(states.get(9)!, fixture.invalid_events.revoked_device_recovery)).rejects.toThrow('revoked');
  });
  it('refuses ancestry substitution, unknown fields, a false digest and a false signature', async () => {
    const event = fixture.events[1];
    await expect(applyManifestEvent(states.get(1)!, { ...event, previous_hash: '00'.repeat(32) })).rejects.toThrow('ancestry');
    expect(() => parseManifestEvent({ ...event, moderator: true })).toThrow('fields');
    await expect(applyManifestEvent(states.get(1)!, { ...event, event_hash: '00'.repeat(32) })).rejects.toThrow('digest');
    await expect(applyManifestEvent(states.get(1)!, { ...event, signature_hex: '00'.repeat(64) })).rejects.toThrow('signature');
  });
  it('rejects rollback and conflicting prior head pins while permitting verified descendants', () => {
    const pin = states.get(5)!.head;
    expect(() => checkManifestPin(pin, states, states.get(9)!)).not.toThrow();
    expect(() => checkManifestPin(pin, states, states.get(4)!)).toThrow('rolled back');
    expect(() => checkManifestPin({ ...pin!, hash: '00'.repeat(32) }, states, states.get(9)!)).toThrow('conflicts');
  });
  it('never seals to a merely certified server-injected outsider', () => {
    const cert = fixture.certificates[1];
    const recipient = { pseudonym_id: cert.pseudonym_id, device_id: cert.device_id, x25519_pub_hex: cert.x25519_pub_hex,
      device_certificate: cert, certificate_sha256: certificateDigest(cert), has_current_wrap: false };
    expect(() => checkManifestRecipient(states.get(1)!, recipient)).toThrow('outside');
    expect(() => checkManifestRecipient(states.get(2)!, recipient)).not.toThrow();
    expect(() => checkManifestRecipient(states.get(2)!, { ...recipient, device_certificate: null })).toThrow('outside');
  });
  it('verifies actual signed wrap receipts only for the exact sealed bytes and historical manifest', () => {
    for (const authorization of fixture.authorizations) {
      const { receipt, wraps } = authorization;
      for (const wrap of wraps) expect(() => verifyWrapAuthorization(receipt, states.get(receipt.manifest_seq)!, wrap, receipt.epoch, receipt.key_id)).not.toThrow();
    }
  });
  it('rejects a substituted envelope, receipt signature, epoch, recipient, or manifest', () => {
    const { receipt, wraps } = fixture.authorizations[0];
    const state = states.get(2)!;
    const ciphertext = Buffer.from(wraps[0].wrapped_key_b64, 'base64'); ciphertext[50] ^= 1;
    expect(() => verifyWrapAuthorization(receipt, state, { ...wraps[0], wrapped_key_b64: ciphertext.toString('base64') }, 1, receipt.key_id)).toThrow('wrap bytes');
    expect(() => verifyWrapAuthorization({ ...receipt, signature_hex: '00'.repeat(64) }, state, wraps[0], 1, receipt.key_id)).toThrow('signature');
    expect(() => verifyWrapAuthorization(receipt, state, wraps[0], 2, receipt.key_id)).toThrow('another channel');
    expect(() => verifyWrapAuthorization(receipt, state, { ...wraps[0], recipient_device_id: '00'.repeat(32) }, 1, receipt.key_id)).toThrow('wrap bytes');
    expect(() => verifyWrapAuthorization(receipt, states.get(3)!, wraps[0], 1, receipt.key_id)).toThrow('manifest');
  });
});
