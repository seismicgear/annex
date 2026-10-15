// @vitest-environment node
import 'fake-indexeddb/auto';
import { readFileSync } from 'node:fs';
import { deleteDB } from 'idb';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ChannelManifestSession, E2eTrustRequiredError, type ManifestHistory, type ManifestTransport } from './e2e-manifest-session';
import { parseManifestPage, parseManifestProjection, parseWrapAuthorization, type ManifestPin } from './e2e-manifest';
import { closeE2eKeyringV2, getChannelManifestPin, retainChannelManifestPin } from './e2e-keyring-v2';

// Real pinned proof computation runs directly in this Node test environment.
vi.mock('./membership-verification', () => import('./membership-verification-core'));

const fixture = JSON.parse(readFileSync(new URL('./__fixtures__/signed-manifest-public.json', import.meta.url), 'utf8'));
const pseudo = fixture.certificates[0].pseudonym_id as string;
beforeEach(async () => { await closeE2eKeyringV2(); await deleteDB('annex-e2e'); });
afterEach(async () => { await closeE2eKeyringV2(); await deleteDB('annex-e2e'); });

function service() {
  let pin: ManifestPin | null = null;
  let last = 9;
  let cancelled = false;
  const transport: ManifestTransport = {
    assertCurrent() { if (cancelled) throw new Error('Credential context changed'); },
    async read(channelId, after) {
      const head = fixture.events[last - 1];
      const events = fixture.events.slice(after, Math.min(after + 2, last));
      return parseManifestPage({ version: 1, authority_identity: fixture.authority_identity, channel_id: channelId,
        head_seq: last, head_hash: head?.event_hash ?? '', genesis_hash: last ? fixture.events[0].event_hash : null,
        events, next_after_seq: after + events.length < last ? after + events.length : null });
    },
    write: vi.fn(async () => { throw new Error('This public fixture contains no signing secrets'); }),
    signer: vi.fn(async () => { throw new Error('This public fixture contains no signing secrets'); }),
    async pin() { return pin; },
    async retain(value, firstUse) {
      if (!pin && !firstUse) throw new Error('First trust requires confirmation');
      pin = value.final.head;
    },
  };
  return { transport, manager: new ChannelManifestSession(pseudo, transport),
    limit: (value: number) => { last = value; }, cancel: () => { cancelled = true; },
    setPin: (value: ManifestPin) => { pin = value; } };
}

describe('verified manifest runtime using genuine public cryptographic fixtures', () => {
  it('verifies every real paged event but does not silently establish first trust or generate a signing key', async () => {
    const test = service();
    const result = await test.manager.inspect(fixture.channel_id, fixture.authority_identity);
    expect(result.history.size).toBe(10);
    expect(result.trusted).toBe(false);
    expect(() => test.manager.requireTrusted(result)).toThrow(E2eTrustRequiredError);
    expect(test.transport.signer).not.toHaveBeenCalled();
    expect(test.transport.write).not.toHaveBeenCalled();
  });
  it('accepts the explicitly displayed genesis and subsequently refuses rollback', async () => {
    const test = service();
    await test.manager.trust(fixture.channel_id, fixture.authority_identity, fixture.events[0].event_hash);
    expect((await test.manager.inspect(fixture.channel_id, fixture.authority_identity)).trusted).toBe(true);
    test.limit(8);
    await expect(test.manager.inspect(fixture.channel_id, fixture.authority_identity)).rejects.toThrow('rolled back');
  });
  it('refuses a fingerprint changed between display and confirmation without saving trust', async () => {
    const test = service();
    await expect(test.manager.trust(fixture.channel_id, fixture.authority_identity, '0'.repeat(64))).rejects.toThrow('fingerprint');
    expect((await test.manager.inspect(fixture.channel_id, fixture.authority_identity)).trusted).toBe(false);
  });
  it('rejects a valid but truncated suffix and a nonadvancing cursor', async () => {
    const test = service();
    const read = test.transport.read;
    test.transport.read = async (channel, after) => ({ ...await read(channel, after), next_after_seq: null });
    await expect(test.manager.inspect(fixture.channel_id, fixture.authority_identity)).rejects.toThrow('incomplete');
    test.transport.read = async (channel, after) => ({ ...await read(channel, after), next_after_seq: after });
    await expect(test.manager.inspect(fixture.channel_id, fixture.authority_identity)).rejects.toThrow('cursor');
  });
  it('refuses a changed advertised head halfway through a genuine chain', async () => {
    const test = service();
    const read = test.transport.read;
    test.transport.read = async (channel, after) => ({ ...await read(channel, after), ...(after ? { head_hash: '0'.repeat(64) } : {}) });
    await expect(test.manager.inspect(fixture.channel_id, fixture.authority_identity)).rejects.toMatchObject({ status: 409 });
  });
  it('does not save trust after identity or server context changes during a read', async () => {
    const test = service();
    const read = test.transport.read;
    test.transport.read = async (channel, after) => { const result = await read(channel, after); test.cancel(); return result; };
    await expect(test.manager.trust(fixture.channel_id, fixture.authority_identity, fixture.events[0].event_hash)).rejects.toThrow('context changed');
    expect(test.transport.signer).not.toHaveBeenCalled();
  });
  it('persists verified ancestry across IndexedDB reopen and rejects a stale concurrent writer', async () => {
    const test = service();
    const full = await test.manager.inspect(fixture.channel_id, fixture.authority_identity);
    const older: ManifestHistory = { ...full, final: full.history.get(2)!, history: new Map([...full.history].filter(([seq]) => seq <= 2)) };
    await expect(retainChannelManifestPin(pseudo, full.history, full.final)).rejects.toThrow('Verify');
    await retainChannelManifestPin(pseudo, older.history, older.final, true);
    await closeE2eKeyringV2();
    expect(await getChannelManifestPin(fixture.authority_identity, pseudo, fixture.channel_id)).toEqual(older.final.head);
    await retainChannelManifestPin(pseudo, full.history, full.final);
    await expect(retainChannelManifestPin(pseudo, older.history, older.final)).rejects.toThrow('rolled back');
    expect(await getChannelManifestPin(fixture.authority_identity, pseudo, fixture.channel_id)).toEqual(full.final.head);
  });
  it('strictly parses actual receipts and rejects malformed fields before signature processing', () => {
    const receipt = fixture.authorizations[0].receipt;
    expect(parseWrapAuthorization(receipt)).toEqual(receipt);
    expect(() => parseWrapAuthorization({ ...receipt, manifest_seq: 0 })).toThrow();
    expect(() => parseWrapAuthorization({ ...receipt, signature_hex: '' })).toThrow();
    expect(() => parseWrapAuthorization({ ...receipt, wrap_digests: [['bad']] })).toThrow();
  });
  it('does not accept an unsigned encryption flag or malformed revoked device in a projection', () => {
    const projection = { head_seq: 1, head_hash: 'a'.repeat(64), genesis_hash: 'a'.repeat(64), join_policy: 'eligible_self_join',
      history_policy: 'all_members', members: [], revoked_devices: [] };
    expect(() => parseManifestProjection(projection)).toThrow();
    expect(() => parseManifestProjection({ ...projection, e2e_enabled: true, revoked_devices: [['a'.repeat(64)]] })).toThrow();
  });
});
