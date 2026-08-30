import { describe, it, expect, vi, beforeEach } from 'vitest';

const mockSetSessionToken = vi.fn();
const mockSetZkProofPayload = vi.fn();
const mockGetCurrentRoot = vi.fn(async () => ({ rootHex: 'ROOT', leafCount: 1 }));
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
  getCurrentRoot: (...args: unknown[]) => mockGetCurrentRoot(...args),
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
  generateMembershipProofV2: vi.fn(async () => ({
    proof: {},
    publicSignals: [],
    nullifierHex: 'n1',
    topicHashHex: 't1',
  })),
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
      { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: 'tok1', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '' },
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
      { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: null, commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '' },
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
      { id: '2', sk: 'def', pseudonymId: null, sessionToken: null, commitmentHex: 'c2', roleCode: 0, nodeId: 'n2', serverSlug: '', leafIndex: null, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '' },
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

    const identity = { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: 'tok1', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '' };
    mockGetIdentity.mockResolvedValueOnce(identity);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().selectIdentity('1');

    expect(mockSetSessionToken).toHaveBeenCalledWith('tok1');
    expect(useIdentityStore.getState().phase).toBe('ready');
  });

  it('selectIdentity clears token for ready identity without token', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    const identity = { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: null, commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '' };
    mockGetIdentity.mockResolvedValueOnce(identity);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().selectIdentity('1');

    expect(mockSetSessionToken).toHaveBeenCalledWith(null);
    expect(useIdentityStore.getState().phase).toBe('ready');
  });

  it('selectIdentity clears token for keys_ready identity', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    const identity = { id: '2', sk: 'def', pseudonymId: null, sessionToken: null, commitmentHex: 'c2', roleCode: 0, nodeId: 'n2', serverSlug: '', leafIndex: null, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '' };
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
      identity: { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: 'tok1', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '' } as Record<string, unknown>,
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

    const imported = { id: '3', sk: 'ghi', pseudonymId: 'p3', sessionToken: 'tok3', commitmentHex: 'c3', roleCode: 0, nodeId: 'n3', serverSlug: 's3', leafIndex: 0, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '' };
    mockImportIdentity.mockResolvedValueOnce(imported);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().importBackup(JSON.stringify(imported));

    expect(mockSetSessionToken).toHaveBeenCalledWith('tok3');
  });

  it('importBackup clears token for keys_ready identity', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();
    mockListIdentities.mockResolvedValueOnce([]);

    const imported = { id: '4', sk: 'jkl', pseudonymId: null, sessionToken: null, commitmentHex: 'c4', roleCode: 0, nodeId: 'n4', serverSlug: '', leafIndex: null, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '' };
    mockImportIdentity.mockResolvedValueOnce(imported);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().importBackup(JSON.stringify(imported));

    expect(mockSetSessionToken).toHaveBeenCalledWith(null);
  });

  it('loadIdentities selects the most recently used ready identity', async () => {
    vi.resetModules();
    mockSetSessionToken.mockClear();

    mockListIdentities.mockResolvedValueOnce([
      { id: '1', sk: 'a', pseudonymId: 'p1', sessionToken: 'tok1', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '2024-01-01', lastUsedAt: '2024-01-01' },
      { id: '2', sk: 'b', pseudonymId: 'p2', sessionToken: 'tok2', commitmentHex: 'c2', roleCode: 0, nodeId: 'n2', serverSlug: 's2', leafIndex: 0, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '2024-01-02', lastUsedAt: '2024-06-01' },
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

    const identity = { id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: 'tok1', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '' };
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
      identity: { id: '1', sk: 'abc', pseudonymId: 'p-new', sessionToken: 'tok', commitmentHex: 'c1', roleCode: 0, nodeId: 'n1', serverSlug: 's2', leafIndex: 0, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }), createdAt: '' } as Record<string, unknown>,
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

/**
 * `cachedProofIsUsable` gates entry into `ready`. It wrapped two very
 * different operations in one `try`: parsing the locally stored proof, and
 * asking the server for its current Merkle root. The single `catch` returned
 * `true` — "the server is unreachable, trust the cache, nothing works offline
 * anyway".
 *
 * That excuse does not apply to the parse. A cached payload that is not JSON
 * is a corrupt credential, not an offline server, and trusting it drops the
 * user into the main UI holding a proof the server will reject — every
 * protected call 403s, with no route back to re-registration because the
 * app believes it is `ready`. The parse must fail closed; only the network
 * call is best-effort.
 */
describe('identity store — a corrupt cached proof must not open the door', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  const base = {
    id: '1', sk: 'abc', pseudonymId: 'p1', sessionToken: 'tok1', commitmentHex: 'c1',
    roleCode: 0, nodeId: 'n1', serverSlug: 's1', leafIndex: 0, createdAt: '',
  };

  it('loadIdentities re-proves when the cached proof is not JSON', async () => {
    vi.resetModules();
    mockListIdentities.mockResolvedValueOnce([
      { ...base, zkProofPayload: 'not json at all' },
    ]);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().loadIdentities();

    expect(useIdentityStore.getState().phase).toBe('keys_ready');
    expect(mockSetZkProofPayload).toHaveBeenCalledWith(null);
  });

  it('loadIdentities re-proves when the cached proof carries a non-string root', async () => {
    vi.resetModules();
    mockListIdentities.mockResolvedValueOnce([
      { ...base, zkProofPayload: JSON.stringify({ root_hex: { nested: true } }) },
    ]);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().loadIdentities();

    expect(useIdentityStore.getState().phase).toBe('keys_ready');
  });

  it('selectIdentity re-proves rather than trusting a truncated payload', async () => {
    vi.resetModules();
    mockGetIdentity.mockResolvedValueOnce({
      ...base, zkProofPayload: '{"root_hex":"ROO',
    });

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().selectIdentity('1');

    expect(useIdentityStore.getState().phase).toBe('keys_ready');
  });

  it('still trusts a well-formed cached proof when the server is unreachable', async () => {
    vi.resetModules();
    mockGetCurrentRoot.mockRejectedValueOnce(new Error('offline'));
    mockListIdentities.mockResolvedValueOnce([
      { ...base, zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }) },
    ]);

    const { useIdentityStore } = await import('./identity');
    await useIdentityStore.getState().loadIdentities();

    expect(useIdentityStore.getState().phase).toBe('ready');
  });
});

/**
 * A backup that will not import has to say so.
 *
 * `importBackup` called `db.importIdentity` with nothing catching it, so a
 * file that is not JSON — or is JSON but not an Annex backup — threw out of
 * the action into an unhandled rejection. `IdentitySetup` renders
 * `{error && ...}` straight from this store, and no failure path ever set
 * it, so choosing the wrong file did nothing whatsoever: no message, no
 * change of state, nothing in the UI at all. That is the first screen of the
 * app, and it is the screen people reach when something has already gone
 * wrong for them and they are trying to restore.
 */
describe('identity store — a backup that cannot be imported', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('reports the failure instead of throwing out of the action', async () => {
    vi.resetModules();
    mockImportIdentity.mockRejectedValueOnce(new SyntaxError('Unexpected token < in JSON'));

    const { useIdentityStore } = await import('./identity');

    await expect(useIdentityStore.getState().importBackup('<html>')).resolves.toBeUndefined();

    const state = useIdentityStore.getState();
    expect(state.error).toMatch(/not a usable Annex backup/i);
    // The detail is what tells someone which file they picked by mistake.
    expect(state.errorDetails).toMatch(/SyntaxError/);
  });

  it('leaves the identity list alone when the import fails', async () => {
    vi.resetModules();
    mockImportIdentity.mockRejectedValueOnce(new Error('not a backup'));

    const { useIdentityStore } = await import('./identity');
    const before = useIdentityStore.getState().storedIdentities;

    await useIdentityStore.getState().importBackup('nonsense');

    expect(useIdentityStore.getState().storedIdentities).toBe(before);
    expect(useIdentityStore.getState().identity).toBeNull();
  });
});

describe('identity store — proof timeout message', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  // `StartupGate` used to append its own hint here — "the first proof can take
  // longer on slower hardware" — behind `error.includes('Proof generation
  // timed out')`. Since this is the only producer of that string and it
  // already carries the same advice, the user read the sentence twice in two
  // wordings. The hint is gone, which makes this message the ONLY place the
  // advice survives: strip the parenthetical here and a user who times out is
  // told to retry with no reason to expect a different result.
  it('tells the user why a retry might succeed, not just to retry', async () => {
    vi.resetModules();
    const zk = await import('@/lib/zk');
    vi.mocked(zk.generateMembershipProofV2).mockRejectedValueOnce(
      new zk.ZkProofTimeoutError('Proof generation timed out after 120s (configured timeout: 120000ms).'),
    );

    const { useIdentityStore } = await import('./identity');
    useIdentityStore.setState({
      identity: {
        id: '1',
        sk: 'ff',
        commitmentHex: 'c1',
        roleCode: 0,
        nodeId: 'n1',
        serverSlug: null,
        leafIndex: null,
        pseudonymId: null,
        sessionToken: null,
        zkProofPayload: null,
        createdAt: '',
      } as never,
    });

    await useIdentityStore.getState().registerWithServer('srv');

    const { phase, error } = useIdentityStore.getState();
    expect(phase).toBe('error');
    expect(error).toMatch(/timed out/i);
    expect(error).toMatch(/first proof can take longer/i);
  });
});
