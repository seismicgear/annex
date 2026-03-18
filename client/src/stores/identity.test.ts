import { describe, it, expect, vi, beforeEach } from 'vitest';

const mockSetSessionToken = vi.fn();
const mockListIdentities = vi.fn(async () => []);
const mockGetIdentity = vi.fn(async () => null);
const mockImportIdentity = vi.fn(async (json: string) => JSON.parse(json));
const mockSaveIdentity = vi.fn(async () => {});

vi.mock('@/lib/api', () => ({
  setSessionToken: (...args: unknown[]) => mockSetSessionToken(...args),
  register: vi.fn(async () => ({ leafIndex: 0, pathElements: [], pathIndexBits: [] })),
  verifyMembership: vi.fn(async () => ({ pseudonymId: 'p1', sessionToken: 'tok1' })),
  getIdentityInfo: vi.fn(async () => ({})),
}));

vi.mock('@/lib/db', () => ({
  listIdentities: (...args: unknown[]) => mockListIdentities(...args),
  getIdentity: (...args: unknown[]) => mockGetIdentity(...args),
  saveIdentity: (...args: unknown[]) => mockSaveIdentity(...args),
  importIdentity: (...args: unknown[]) => mockImportIdentity(...args),
  exportIdentity: vi.fn(() => '{}'),
}));

vi.mock('@/lib/zk', () => ({
  initPoseidon: vi.fn(async () => {}),
  generateSecretKey: vi.fn(() => BigInt(42)),
  generateNodeId: vi.fn(() => 'node1'),
  computeCommitment: vi.fn(async () => 'commit1'),
  generateMembershipProof: vi.fn(async () => ({ proof: {}, publicSignals: [] })),
  cancelMembershipProofGeneration: vi.fn(async () => {}),
  isProofGenerationInFlight: vi.fn(() => false),
  ZkProofAssetsError: class extends Error {},
  ZkProofTimeoutError: class extends Error {},
  ZkProofInFlightError: class extends Error {},
  ZkProofCancelledError: class extends Error {},
}));

describe('identity store — API auth state sync', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('loadIdentities sets token when ready identity has a sessionToken', async () => {
    mockListIdentities.mockResolvedValueOnce([
      { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: 'tok1', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, createdAt: '' },
    ]);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().loadIdentities();

    expect(mockSetSessionToken).toHaveBeenCalledWith('tok1');
    expect(useIdentityStore.getState().phase).toBe('ready');
  });

  it('loadIdentities clears token when ready identity has no sessionToken', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    mockListIdentities.mockResolvedValueOnce([
      { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: null, commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, createdAt: '' },
    ]);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().loadIdentities();

    expect(mockSetSessionToken).toHaveBeenCalledWith(null);
    expect(useIdentityStore.getState().phase).toBe('ready');
  });

  it('loadIdentities clears token when falling back to keys_ready', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    mockListIdentities.mockResolvedValueOnce([
      { id: '2', sk: 'def', pseudonymId: null, sessionToken: null, commitmentHex: 'c2', roleCode: 0, nodeId: 'n2', serverSlug: '', leafIndex: null, createdAt: '' },
    ]);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().loadIdentities();

    expect(mockSetSessionToken).toHaveBeenCalledWith(null);
    expect(useIdentityStore.getState().phase).toBe('keys_ready');
  });

  it('loadIdentities clears token when no identities exist', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    mockListIdentities.mockResolvedValueOnce([]);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().loadIdentities();

    expect(mockSetSessionToken).toHaveBeenCalledWith(null);
    expect(useIdentityStore.getState().phase).toBe('uninitialized');
  });

  it('selectIdentity sets token for ready identity with token', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    const identity = { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: 'tok1', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, createdAt: '' };
    mockGetIdentity.mockResolvedValueOnce(identity);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().selectIdentity('1');

    expect(mockSetSessionToken).toHaveBeenCalledWith('tok1');
    expect(useIdentityStore.getState().phase).toBe('ready');
  });

  it('selectIdentity clears token for ready identity without token', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    const identity = { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: null, commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, createdAt: '' };
    mockGetIdentity.mockResolvedValueOnce(identity);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().selectIdentity('1');

    expect(mockSetSessionToken).toHaveBeenCalledWith(null);
    expect(useIdentityStore.getState().phase).toBe('ready');
  });

  it('selectIdentity clears token for keys_ready identity', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    const identity = { id: '2', sk: 'def', pseudonymId: null, sessionToken: null, commitmentHex: 'c2', roleCode: 0, nodeId: 'n2', serverSlug: '', leafIndex: null, createdAt: '' };
    mockGetIdentity.mockResolvedValueOnce(identity);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().selectIdentity('2');

    expect(mockSetSessionToken).toHaveBeenCalledWith(null);
    expect(useIdentityStore.getState().phase).toBe('keys_ready');
  });

  it('logout clears the session token', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    const { useIdentityStore } = await import('./identity');
    useIdentityStore.getState().logout();

    expect(mockSetSessionToken).toHaveBeenCalledWith(null);
    expect(useIdentityStore.getState().phase).toBe('uninitialized');
  });

  it('importBackup sets token for ready identity', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();
    mockListIdentities.mockResolvedValueOnce([]);

    const imported = { id: '3', sk: 'ghi', pseudonymId: 'p3', sessionToken: 'tok3', commitmentHex: 'c3', roleCode: 0, nodeId: 'n3', serverSlug: 's3', leafIndex: 0, createdAt: '' };
    mockImportIdentity.mockResolvedValueOnce(imported);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().importBackup(JSON.stringify(imported));

    expect(mockSetSessionToken).toHaveBeenCalledWith('tok3');
  });

  it('importBackup clears token for keys_ready identity', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();
    mockListIdentities.mockResolvedValueOnce([]);

    const imported = { id: '4', sk: 'jkl', pseudonymId: null, sessionToken: null, commitmentHex: 'c4', roleCode: 0, nodeId: 'n4', serverSlug: '', leafIndex: null, createdAt: '' };
    mockImportIdentity.mockResolvedValueOnce(imported);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().importBackup(JSON.stringify(imported));

    expect(mockSetSessionToken).toHaveBeenCalledWith(null);
  });
});
