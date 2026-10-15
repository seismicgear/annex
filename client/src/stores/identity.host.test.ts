import 'fake-indexeddb/auto';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { StoredIdentity } from '@/types';
import * as db from '@/lib/db';
import * as zk from '@/lib/zk';
import { createHash } from 'node:crypto';
import { useIdentityStore } from './identity';

vi.mock('@/lib/zk', async () => ({
  computeTopicNullifierV2: (await vi.importActual<typeof import('@/lib/zk')>('@/lib/zk')).computeTopicNullifierV2,
  initPoseidon: vi.fn(async () => {}), generateSecretKey: () => 42n, generateNodeId: () => 7,
  computeCommitment: vi.fn(async () => 'new-commitment'),
  cancelMembershipProofGeneration: vi.fn(async () => {}), isProofGenerationInFlight: () => false,
}));
vi.mock('@/lib/api', () => ({
  setSessionToken: vi.fn(), setZkProofPayload: vi.fn(),
  getCurrentRoot: vi.fn(async () => ({ rootHex: 'host-root' })),
}));
vi.mock('./voice', () => ({ useVoiceStore: { getState: () => ({ forceReset: vi.fn() }) } }));

const record = (id: string, server: string, pseudonym: string | null): StoredIdentity => ({
  id, sk: '1234', roleCode: 1, nodeId: 1, commitmentHex: 'commitment', serverSlug: server,
  pseudonymId: pseudonym, sessionToken: pseudonym ? 'token-' + id : null, leafIndex: pseudonym ? 0 : null,
  zkProofPayload: pseudonym ? JSON.stringify({ root_hex: 'host-root' }) : null,
  createdAt: '2026-10-15T00:00:00Z',
});

describe('host founder selection with actual local identity storage', () => {
  beforeEach(() => { useIdentityStore.setState({ identity: null, storedIdentities: [], phase: 'uninitialized', error: null }); });
  afterEach(async () => { await db.clearAllDatabases(); });

  it('restores the local founder after the user last visited a remote server', async () => {
    const founder = record('founder', 'host', 'local-founder');
    const remote = record('remote', 'elsewhere', 'remote-member');
    await db.saveIdentity(founder); await db.saveIdentity(remote);
    useIdentityStore.setState({ identity: remote, phase: 'ready' });
    await useIdentityStore.getState().prepareHostIdentity('host', 'local-founder');
    expect(useIdentityStore.getState().identity?.id).toBe('founder');
    expect(useIdentityStore.getState().phase).toBe('ready');
    expect(await db.getIdentity('remote')).toEqual(remote);
  });

  it('distinguishes legacy hosts that share the default slug by their actual founder', async () => {
    const local = record('local', 'default', 'local-founder');
    const remote = record('remote', 'default', 'remote-founder');
    await db.saveIdentity(local); await db.saveIdentity(remote);
    useIdentityStore.setState({ identity: remote, phase: 'ready' });
    await useIdentityStore.getState().prepareHostIdentity('default', 'local-founder');
    expect(useIdentityStore.getState().identity?.id).toBe('local');
  });

  it('creates a fresh host record without overwriting another server membership', async () => {
    const remote = record('remote', 'elsewhere', 'remote-member');
    await db.saveIdentity(remote);
    useIdentityStore.setState({ identity: remote, phase: 'ready' });
    await useIdentityStore.getState().prepareHostIdentity('fresh-host', null);
    const next = useIdentityStore.getState().identity!;
    expect(next.id).not.toBe(remote.id);
    expect(next).toMatchObject({ sk: remote.sk, serverSlug: 'fresh-host', pseudonymId: null, sessionToken: null });
    expect(useIdentityStore.getState().phase).toBe('keys_ready');
    expect(await db.getIdentity('remote')).toEqual(remote);
  });

  it('generates identity keys automatically for a brand-new host', async () => {
    await useIdentityStore.getState().prepareHostIdentity('fresh-host', null);
    expect(useIdentityStore.getState().identity).toMatchObject({ serverSlug: 'fresh-host', pseudonymId: null, sk: '2a' });
    expect(await db.listIdentities()).toHaveLength(1);
  });

  it('waits for a single key generation when setup actions overlap', async () => {
    let finishKeys!: () => void;
    vi.mocked(zk.initPoseidon).mockImplementationOnce(() => new Promise(resolve => { finishKeys = resolve; }));
    const first = useIdentityStore.getState().generateLocalKeys(1);
    const host = useIdentityStore.getState().prepareHostIdentity('host', null);
    expect(useIdentityStore.getState().phase).toBe('generating');
    finishKeys();
    await Promise.all([first, host]);
    expect(await db.listIdentities()).toHaveLength(1);
    expect(useIdentityStore.getState().identity).toMatchObject({ serverSlug: 'host', sk: '2a' });
  });

  it('uses an existing unbound identity without duplicating it', async () => {
    const unbound = record('keys', '', null);
    await db.saveIdentity(unbound); useIdentityStore.setState({ identity: unbound, phase: 'keys_ready' });
    await useIdentityStore.getState().prepareHostIdentity('fresh-host', null);
    expect(useIdentityStore.getState().identity?.id).toBe('keys');
    expect(await db.listIdentities()).toHaveLength(1);
  });

  it('reuses pending host keys on retry instead of accumulating identities', async () => {
    const pending = record('pending', 'host', null);
    await db.saveIdentity(pending); useIdentityStore.setState({ identity: pending, phase: 'error' });
    await useIdentityStore.getState().prepareHostIdentity('host', null);
    expect(useIdentityStore.getState().identity?.id).toBe('pending');
    expect(await db.listIdentities()).toHaveLength(1);
  });

  it('recovers the original founder after admission succeeded but its response was lost', async () => {
    const pending = record('pending', 'host', null);
    const topic = 'annex:server:host:v2';
    const nullifier = await zk.computeTopicNullifierV2(BigInt('0x' + pending.sk), topic);
    const founderPseudonym = createHash('sha256').update(`${topic}:${nullifier}`).digest('hex');
    await db.saveIdentity(pending); useIdentityStore.setState({ identity: pending, phase: 'error' });
    await useIdentityStore.getState().prepareHostIdentity('host', founderPseudonym);
    expect(useIdentityStore.getState().identity).toMatchObject({ id: 'pending', pseudonymId: founderPseudonym, sessionToken: null });
    expect(useIdentityStore.getState().phase).toBe('keys_ready');
    expect(await db.listIdentities()).toHaveLength(1);
  });

  it('can recover matching founder keys from another server backup without rewriting that membership', async () => {
    const remote = record('remote', 'elsewhere', 'remote-member');
    const topic = 'annex:server:host:v2';
    const nullifier = await zk.computeTopicNullifierV2(BigInt('0x' + remote.sk), topic);
    const founderPseudonym = createHash('sha256').update(`${topic}:${nullifier}`).digest('hex');
    await db.saveIdentity(remote); useIdentityStore.setState({ identity: remote, phase: 'ready' });
    await useIdentityStore.getState().prepareHostIdentity('host', founderPseudonym);
    expect(useIdentityStore.getState().identity).toMatchObject({ serverSlug: 'host', pseudonymId: founderPseudonym, sessionToken: null });
    expect(useIdentityStore.getState().identity?.id).not.toBe(remote.id);
    expect(await db.getIdentity(remote.id)).toEqual(remote);
    expect(await db.listIdentities()).toHaveLength(2);
  });

  it('preserves an existing server when its founder keys are missing', async () => {
    const remote = record('remote', 'elsewhere', 'remote-member');
    await db.saveIdentity(remote); useIdentityStore.setState({ identity: remote, phase: 'ready' });
    await expect(useIdentityStore.getState().prepareHostIdentity('host', 'missing-founder')).rejects.toThrow('Restore that founder');
    expect(await db.listIdentities()).toEqual([remote]);
  });
});
