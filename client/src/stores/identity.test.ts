import { describe, it, expect, vi, beforeEach } from 'vitest';

const mockSetSessionToken = vi.fn();
const mockSetZkProofPayload = vi.fn();
const mockListIdentities = vi.fn(async () => []);
const mockGetIdentity = vi.fn(async () => null);
const mockImportIdentity = vi.fn(async (json: string) => JSON.parse(json));
const mockSaveIdentity = vi.fn(async () => {});

class MockApiError extends Error {
  status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }
}

vi.mock('@/lib/api', () => ({
  setSessionToken: (...args: unknown[]) => mockSetSessionToken(...args),
  setZkProofPayload: (...args: unknown[]) => mockSetZkProofPayload(...args),
  register: vi.fn(async () => ({ leafIndex: 0, pathElements: [], pathIndexBits: [] })),
  verifyMembership: vi.fn(async () => ({ pseudonymId: 'p1', sessionToken: 'tok1' })),
  getIdentityInfo: vi.fn(async () => ({})),
  ApiError: MockApiError,
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

const mockLeaveCall = vi.fn(async () => {});
const mockForceReset = vi.fn();
vi.mock('./voice', () => ({
  useVoiceStore: {
    getState: () => ({
      connectedChannelId: 'ch-1',
      voiceToken: 'voice-tok-1',
      leaveCall: (...args: unknown[]) => mockLeaveCall(...args),
      forceReset: (...args: unknown[]) => mockForceReset(...args),
    }),
  },
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

  it('logout tears down voice state before clearing identity', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();
    mockLeaveCall.mockClear();
    mockForceReset.mockClear();

    const { useIdentityStore } = await import('./identity');

    // Set up an identity with a pseudonymId (simulating an active session)
    useIdentityStore.setState({
      identity: { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: 'tok1', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, createdAt: '' } as Record<string, unknown>,
      phase: 'ready',
    });

    useIdentityStore.getState().logout();

    // Voice teardown should happen before token is cleared
    expect(mockLeaveCall).toHaveBeenCalledWith('p1');
    expect(mockForceReset).toHaveBeenCalled();
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

  it('loadIdentities selects the most recently used ready identity', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    mockListIdentities.mockResolvedValueOnce([
      { id: '1', sk: 'a', pseudonymId: 'p1', sessionToken: 'tok1', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, createdAt: '2024-01-01', lastUsedAt: '2024-01-01' },
      { id: '2', sk: 'b', pseudonymId: 'p2', sessionToken: 'tok2', commitmentHex: 'c2', roleCode: 0, nodeId: 'n2', serverSlug: 's2', leafIndex: 0, createdAt: '2024-01-02', lastUsedAt: '2024-06-01' },
    ]);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().loadIdentities();

    // Should select the identity with the most recent lastUsedAt
    expect(useIdentityStore.getState().identity?.id).toBe('2');
    expect(mockSetSessionToken).toHaveBeenCalledWith('tok2');
  });

  it('selectIdentity clears permissions from previous identity', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    const identity = { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: 'tok1', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, createdAt: '' };
    mockGetIdentity.mockResolvedValueOnce(identity);

    const { useIdentityStore } = await import('./identity');

    // Pre-set permissions from a previous server
    useIdentityStore.setState({
      permissions: { pseudonymId: 'old-p', participantType: 'HUMAN', active: true, capabilities: { can_voice: true, can_moderate: true, can_invite: true, can_federate: false, can_bridge: false } } as Record<string, unknown>,
      permissionsStatus: 'ready',
      permissionsPseudonymId: 'old-p',
    });

    await useIdentityStore.getState().selectIdentity('1');

    expect(useIdentityStore.getState().permissions).toBeNull();
    expect(useIdentityStore.getState().permissionsStatus).toBe('idle');
    expect(useIdentityStore.getState().permissionsPseudonymId).toBeNull();
  });

  it('loadPermissions clears stale permissions when pseudonym changed and request fails', async () => {
    vi.resetModules();

    const apiModule = await import('@/lib/api');
    vi.mocked(apiModule.getIdentityInfo).mockRejectedValueOnce(new Error('network error'));

    const { useIdentityStore } = await import('./identity');

    // Simulate: permissions from server A (pseudonym 'p-old'), now on server B (pseudonym 'p-new')
    useIdentityStore.setState({
      identity: { id: '1', sk: 'abc', pseudonymId: 'p-new', sessionToken: 'tok', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's2', leafIndex: 0, createdAt: '' } as Record<string, unknown>,
      permissions: { pseudonymId: 'p-old', capabilities: { can_voice: true, can_moderate: true } } as Record<string, unknown>,
      permissionsStatus: 'ready',
      permissionsPseudonymId: 'p-old',
    });

    await useIdentityStore.getState().loadPermissions();

    // Permissions should be cleared because the pseudonym changed
    expect(useIdentityStore.getState().permissions).toBeNull();
    expect(useIdentityStore.getState().permissionsStatus).toBe('error');
  });
});
