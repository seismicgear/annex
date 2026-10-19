import { describe, expect, it, vi } from 'vitest';
import { arrayWalletMessage, portableArrayIdentity, verifiedArrayIdentity } from './array-wallet';

const compute = vi.hoisted(() => vi.fn(async () => 'a'.repeat(64)));
vi.mock('@/lib/zk', () => ({ computeCommitment: compute }));

const identity = {
  sk: '1', roleCode: 1, nodeId: 0, commitmentHex: 'a'.repeat(64),
  pseudonymId: null, serverSlug: '', createdAt: '2026-10-19T00:00:00Z',
};

describe('Array portable Annex identity', () => {
  it('preserves complete recovery fields and excludes source sessions, proofs and local IDs', () => {
    const backup = portableArrayIdentity({ ...identity, id: 'local-id', sessionToken: 'source-session', zkProofPayload: 'source-proof', leafIndex: 4 });
    expect(backup).toEqual(identity);
    expect(JSON.stringify(backup)).not.toContain('source-');
  });

  it.each([
    { sk: '0' }, { sk: '0x1' }, { sk: 'f'.repeat(64) }, { sk: '1'.repeat(65) },
    { roleCode: 0 }, { roleCode: 256 }, { nodeId: -1 }, { nodeId: Number.MAX_SAFE_INTEGER + 1 },
    { commitmentHex: 'short' }, { serverSlug: '界'.repeat(171) }, { pseudonymId: {} },
  ])('rejects unusable recovery material (%j)', changed => {
    expect(() => portableArrayIdentity({ ...identity, ...changed })).toThrow('not a usable Annex identity backup');
  });

  it('accepts short scalars, legacy zero node IDs and a full 512-byte server context', () => {
    expect(portableArrayIdentity({ ...identity, serverSlug: 'a'.repeat(512) }).sk).toBe('1');
  });

  it('recomputes the commitment and rejects mismatched keys before any import', async () => {
    await expect(verifiedArrayIdentity(identity)).resolves.toEqual(identity);
    expect(compute).toHaveBeenCalledWith(1n, 1, 0);
    await expect(verifiedArrayIdentity({ ...identity, commitmentHex: 'b'.repeat(64) })).rejects.toThrow('does not match its cryptographic identity');
  });

  it('never displays unexpected native/remote diagnostic payloads', () => {
    expect(arrayWalletMessage('private-provider-secret', 'Safe fallback')).toBe('Safe fallback');
    expect(arrayWalletMessage('Unlock the credential wallet in Array and try again.', 'Fallback')).toContain('Unlock');
  });
});
