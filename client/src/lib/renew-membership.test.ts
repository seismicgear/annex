import { beforeEach, describe, expect, it, vi } from 'vitest';
const mocked = vi.hoisted(() => ({
  serial: 1, token: 'same-session', proof: '', phase: 'ready', session: 'connected',
  identity: { id: 'identity', sk: '01', roleCode: 1, nodeId: 7, commitmentHex: '02', pseudonymId: 'owner', serverSlug: 'server', leafIndex: 0, zkProofPayload: '' },
  path: vi.fn(), prove: vi.fn(), save: vi.fn(), ownership: vi.fn(), install: vi.fn(), set: vi.fn(), busy: vi.fn(),
}));
vi.mock('@/api/core', () => ({ getCredentialContext: () => ({ serial: mocked.serial, baseUrl: '', pseudonymId: 'owner' }), getSessionToken: () => mocked.token,
  getZkProofPayload: () => mocked.proof, isCredentialContextCurrent: (context: { serial: number }) => context.serial === mocked.serial,
  setZkProofPayload: mocked.install, StaleCredentialContextError: class extends Error {},
}));
vi.mock('@/api/identity', () => ({ getMerklePath: mocked.path }));
vi.mock('@/stores/identity', () => ({ useIdentityStore: { getState: () => ({ identity: mocked.identity, phase: mocked.phase }), setState: mocked.set } }));
vi.mock('@/stores/channels', () => ({ useChannelsStore: { getState: () => ({ wsSessionState: mocked.session }) } }));
vi.mock('./db', () => ({ saveIdentity: mocked.save }));
vi.mock('./session-ownership', () => ({ assertSessionOwnership: mocked.ownership }));
vi.mock('./zk', () => ({ generateMembershipProofV2: mocked.prove, isProofGenerationInFlight: mocked.busy }));
import { renewMembershipAssertion } from './renew-membership';
beforeEach(() => {
  vi.clearAllMocks(); mocked.serial = 1; mocked.token = 'same-session'; mocked.phase = 'ready'; mocked.session = 'connected';
  mocked.proof = JSON.stringify({ protocolVersion: 'v2', commitment_hex: '02', topic: 'annex:server:server:v2', publicSignals: ['1', '2', '3', '4', '5'] });
  mocked.path.mockResolvedValue({ rootHex: '06', leafIndex: 0, pathElements: ['07'], pathIndexBits: [0] });
  mocked.prove.mockResolvedValue({ proof: { actual: 'proof-output' }, publicSignals: ['6', '2', '3', '4', '5'] });
  mocked.save.mockResolvedValue(undefined); mocked.busy.mockReturnValue(false); mocked.ownership.mockImplementation(() => {});
});
describe('assertion-only renewal', () => {
  it('re-proves the current Merkle path with the spent challenge and preserves all identity/topic/nullifier bindings', async () => {
    await renewMembershipAssertion();
    expect(mocked.prove).toHaveBeenCalledWith({ sk: 1n, roleCode: 1, nodeId: 7, leafIndex: 0, pathElements: ['07'], pathIndexBits: [0] }, 'annex:server:server:v2', '5'.padStart(64, '0'));
    expect(JSON.parse(mocked.install.mock.calls[0][0])).toMatchObject({ root_hex: '06', commitment_hex: '02', publicSignals: ['6', '2', '3', '4', '5'] });
    expect(mocked.token).toBe('same-session'); expect(mocked.set.mock.calls[0][0]).not.toHaveProperty('phase');
  });
  it.each([['6', '9', '3', '4', '5'], ['6', '2', '8', '4', '5'], ['6', '2', '3', '9', '5'], ['6', '2', '3', '4', '9']])('rejects changed proof identity binding %j', async (...signals) => {
    mocked.prove.mockResolvedValue({ proof: {}, publicSignals: signals });
    await expect(renewMembershipAssertion()).rejects.toThrow('identity binding');
    expect(mocked.install).not.toHaveBeenCalled(); expect(mocked.save).not.toHaveBeenCalled();
  });
  it.each(['context', 'token', 'revoked', 'logout', 'takeover'] as const)('does not publish a late proof after %s', async reason => {
    mocked.prove.mockImplementation(async () => {
      if (reason === 'context') mocked.serial++;
      if (reason === 'token') mocked.token = 'new-epoch';
      if (reason === 'revoked') mocked.session = 'revoked';
      if (reason === 'logout') mocked.phase = 'keys_ready';
      if (reason === 'takeover') mocked.ownership.mockImplementation(() => { throw new Error('Taken over'); });
      return { proof: {}, publicSignals: ['6', '2', '3', '4', '5'] };
    });
    await expect(renewMembershipAssertion()).rejects.toThrow();
    expect(mocked.install).not.toHaveBeenCalled(); expect(mocked.set).not.toHaveBeenCalled(); expect(mocked.save).not.toHaveBeenCalled();
  });
});
