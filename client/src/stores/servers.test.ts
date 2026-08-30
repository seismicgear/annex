import { describe, it, expect, vi, beforeEach } from 'vitest';

// Track mock calls for assertions
const mockConnectWs = vi.fn();
const mockSelectIdentity = vi.fn(async () => {});
const mockLoadPermissions = vi.fn(async () => {});
const mockSetApiBaseUrl = vi.fn();
const mockSaveServer = vi.fn(async () => {});
const mockListServers = vi.fn(async () => []);
const mockGetServerByIdentityId = vi.fn(async () => null);
const mockRemoveServer = vi.fn(async () => {});
const mockUpdateCachedSummary = vi.fn(async () => {});
const mockGetServerSummary = vi.fn(async () => ({ slug: 'test', label: 'Test' }));
const mockCreateServerEntry = vi.fn(
  (baseUrl: string, slug: string, label: string, identityId: string) => ({
    id: `gen-${Math.random().toString(36).slice(2)}`,
    baseUrl,
    slug,
    label,
    identityId,
    personaId: null,
    accentColor: '#e63946',
    vrpTopic: `annex:server:${slug}:v1`,
    lastConnectedAt: new Date().toISOString(),
    cachedSummary: null,
  }),
);

// Mock dependencies before importing the store
vi.mock('@/lib/servers', () => ({
  listServers: (...args: unknown[]) => mockListServers(...args),
  saveServer: (...args: unknown[]) => mockSaveServer(...args),
  getServerByIdentityId: (...args: unknown[]) => mockGetServerByIdentityId(...args),
  createServerEntry: (...args: unknown[]) => (mockCreateServerEntry as (...a: unknown[]) => unknown)(...args),
  removeServer: (...args: unknown[]) => mockRemoveServer(...args),
  updateCachedSummary: (...args: unknown[]) => mockUpdateCachedSummary(...args),
  getServerBySlug: vi.fn(async () => undefined),
}));

const mockGetServerImage = vi.fn(async () => ({ image_url: null }));
const mockGetRemoteServerSummary = vi.fn(async () => ({
  slug: 'remote',
  label: 'Remote Server',
  total_active_members: 5,
}));
let mockCurrentApiBaseUrl = '';
vi.mock('@/lib/api', () => ({
  setApiBaseUrl: (...args: unknown[]) => { mockCurrentApiBaseUrl = args[0] as string; mockSetApiBaseUrl(...args); },
  getApiBaseUrl: () => mockCurrentApiBaseUrl,
  getServerImage: (...args: unknown[]) => mockGetServerImage(...args),
  getServerSummary: (...args: unknown[]) => mockGetServerSummary(...args),
  getRemoteServerSummary: (...args: unknown[]) => mockGetRemoteServerSummary(...args),
  resolveUrl: (url: string) => url,
}));

const mockCloneForServer = vi.fn(async (): Promise<string | null> => 'cloned-identity-1');
vi.mock('./identity', () => ({
  useIdentityStore: Object.assign(
    () => ({}),
    {
      getState: () => ({
        identity: { pseudonymId: 'p1', sessionToken: 'session-tok-1' },
        selectIdentity: (...args: unknown[]) => mockSelectIdentity(...args),
        loadPermissions: (...args: unknown[]) => mockLoadPermissions(...args),
        cloneForServer: () => mockCloneForServer(),
      }),
      setState: vi.fn(),
    },
  ),
}));

vi.mock('./channels', () => ({
  useChannelsStore: {
    getState: () => ({
      disconnectWs: vi.fn(),
      connectWs: (...args: unknown[]) => mockConnectWs(...args),
      loadChannels: vi.fn(async () => {}),
      resetServerState: vi.fn(),
    }),
  },
}));

vi.mock('./voice', () => ({
  useVoiceStore: {
    getState: () => ({
      connectedChannelId: null,
      voiceToken: null,
      leaveCall: vi.fn(async () => {}),
      forceReset: vi.fn(),
    }),
  },
}));

describe('servers store', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mockListServers.mockResolvedValue([]);
    mockGetServerByIdentityId.mockResolvedValue(null);
    mockCreateServerEntry.mockImplementation(
      (baseUrl: string, slug: string, label: string, identityId: string) => ({
        id: `gen-${Math.random().toString(36).slice(2)}`,
        baseUrl,
        slug,
        label,
        identityId,
        personaId: null,
        accentColor: '#e63946',
        vrpTopic: `annex:server:${slug}:v1`,
        lastConnectedAt: new Date().toISOString(),
        cachedSummary: null,
      }),
    );
    mockUpdateCachedSummary.mockReset();
    mockGetServerSummary.mockReset();
    mockGetServerSummary.mockResolvedValue({ slug: 'test', label: 'Test' });
  });

  it('fetchServerImage sets serverImageUrl to null on failure', async () => {
    const { useServersStore } = await import('./servers');
    mockGetServerImage.mockRejectedValueOnce(new Error('404'));

    useServersStore.getState().setServerImageUrl('https://old-server/image.png');
    expect(useServersStore.getState().serverImageUrl).toBe('https://old-server/image.png');

    await useServersStore.getState().fetchServerImage();
    expect(useServersStore.getState().serverImageUrl).toBeNull();
  });

  it('switchServer does not proceed for servers without identityId', async () => {
    const { useServersStore } = await import('./servers');

    useServersStore.setState({
      servers: [{ id: 'pending-1', baseUrl: 'https://remote.example.com', slug: 'remote', label: 'Remote', identityId: '', cachedSummary: null, personaId: null, accentColor: '#e63946', lastConnectedAt: null } as Record<string, unknown>],
      activeServerId: null,
    });

    await useServersStore.getState().switchServer('pending-1');
    expect(useServersStore.getState().activeServerId).toBeNull();
  });

  it('fetchServerImage sets null when server has no image', async () => {
    const { useServersStore } = await import('./servers');
    mockGetServerImage.mockResolvedValueOnce({ image_url: null });

    useServersStore.getState().setServerImageUrl('https://old/img.png');
    await useServersStore.getState().fetchServerImage();
    expect(useServersStore.getState().serverImageUrl).toBeNull();
  });

  // ── saveCurrentServer with base URL ──

  it('saveCurrentServer stores remote base URL instead of empty string', async () => {
    const { useServersStore } = await import('./servers');

    // Verify the store has no pre-existing server state that could short-circuit
    useServersStore.setState({ servers: [], activeServerId: null });

    await useServersStore.getState().saveCurrentServer('id-1', 'remote', 'Remote', 'https://remote.example.com');

    // saveServer should have been called with the server that has the remote baseUrl
    expect(mockSaveServer).toHaveBeenCalledWith(
      expect.objectContaining({
        baseUrl: 'https://remote.example.com',
        identityId: 'id-1',
      }),
    );
  });

  it('saveCurrentServer preserves empty base URL for local server', async () => {
    const { useServersStore } = await import('./servers');

    await useServersStore.getState().saveCurrentServer('id-1', 'local', 'Local', '');

    expect(mockCreateServerEntry).toHaveBeenCalledWith(
      '', 'local', 'Local', 'id-1',
    );
  });

  it('saveCurrentServer updates existing placeholder instead of creating duplicate', async () => {
    const { useServersStore } = await import('./servers');

    // Set up a placeholder entry from addRemoteServer
    const placeholder = {
      id: 'placeholder-1',
      baseUrl: 'https://remote.example.com',
      slug: 'remote',
      label: 'Remote Server',
      identityId: '', // pending registration
      personaId: null,
      accentColor: '#646cff',
      vrpTopic: 'annex:server:remote:v1',
      lastConnectedAt: '2025-01-01T00:00:00Z',
      cachedSummary: null,
    };
    useServersStore.setState({ servers: [placeholder] });

    await useServersStore.getState().saveCurrentServer(
      'identity-abc', 'remote', 'Remote Server', 'https://remote.example.com',
    );

    // Should NOT have called createServerEntry (reused placeholder)
    expect(mockCreateServerEntry).not.toHaveBeenCalled();
    // Should have saved the updated placeholder with the identity
    expect(mockSaveServer).toHaveBeenCalledWith(
      expect.objectContaining({
        id: 'placeholder-1',
        identityId: 'identity-abc',
        baseUrl: 'https://remote.example.com',
      }),
    );
  });

  // ── switchServer passes session token ──

  it('switchServer passes session token to connectWs', async () => {
    const { useServersStore } = await import('./servers');

    const server = {
      id: 'srv-1',
      baseUrl: 'https://remote.example.com',
      slug: 'remote',
      label: 'Remote',
      identityId: 'id-1',
      personaId: null,
      accentColor: '#e63946',
      vrpTopic: 'annex:server:remote:v1',
      lastConnectedAt: '2025-01-01T00:00:00Z',
      cachedSummary: null,
    };
    useServersStore.setState({ servers: [server], activeServerId: null });
    mockListServers.mockResolvedValue([server]);

    await useServersStore.getState().switchServer('srv-1');

    expect(mockConnectWs).toHaveBeenCalledWith('p1', 'https://remote.example.com', 'session-tok-1');
    expect(mockSetApiBaseUrl).toHaveBeenCalledWith('https://remote.example.com');
    expect(mockSelectIdentity).toHaveBeenCalledWith('id-1');
  });

  // ── removeServer ordering ──

  it('removeServer does not pre-set activeServerId before switchServer', async () => {
    const { useServersStore } = await import('./servers');

    const server1 = {
      id: 'srv-1', baseUrl: '', slug: 'a', label: 'A', identityId: 'id-1',
      personaId: null, accentColor: '#e63946', vrpTopic: '', lastConnectedAt: '2025-01-01', cachedSummary: null,
    };
    const server2 = {
      id: 'srv-2', baseUrl: 'https://b.example.com', slug: 'b', label: 'B', identityId: 'id-2',
      personaId: null, accentColor: '#646cff', vrpTopic: '', lastConnectedAt: '2025-01-02', cachedSummary: null,
    };

    useServersStore.setState({ servers: [server1, server2], activeServerId: 'srv-1' });
    mockListServers.mockResolvedValue([server2]);

    await useServersStore.getState().removeServer('srv-1');

    // switchServer should have been called and done the full connect path
    expect(mockSetApiBaseUrl).toHaveBeenCalledWith('https://b.example.com');
    expect(mockSelectIdentity).toHaveBeenCalledWith('id-2');
    expect(mockConnectWs).toHaveBeenCalled();
  });

  // ── addRemoteServer + registration flow ──

  it('cleanupFailedRegistration removes placeholder and clears pendingRegistrationServerId', async () => {
    const { useServersStore } = await import('./servers');

    const placeholder = {
      id: 'pending-fail',
      baseUrl: 'https://failed.example.com',
      slug: 'failed',
      label: 'Failed Server',
      identityId: '',
      personaId: null,
      accentColor: '#e63946',
      vrpTopic: 'annex:server:failed:v1',
      lastConnectedAt: null,
      cachedSummary: null,
    };

    useServersStore.setState({
      servers: [placeholder as Record<string, unknown>],
      pendingRegistrationServerId: 'pending-fail',
    });

    // After cleanup, the list should be empty
    mockListServers.mockResolvedValueOnce([]);

    await useServersStore.getState().cleanupFailedRegistration();

    expect(mockRemoveServer).toHaveBeenCalledWith('pending-fail');
    expect(useServersStore.getState().pendingRegistrationServerId).toBeNull();
  });

  it('cleanupFailedRegistration clears state even with no target server', async () => {
    const { useServersStore } = await import('./servers');

    useServersStore.setState({ pendingRegistrationServerId: null });
    await useServersStore.getState().cleanupFailedRegistration();
    expect(useServersStore.getState().pendingRegistrationServerId).toBeNull();
    // removeServer should not have been called
    expect(mockRemoveServer).not.toHaveBeenCalled();
  });

  it('addRemoteServer creates placeholder then saveCurrentServer fills it', async () => {
    const { useServersStore } = await import('./servers');

    // Step 1: addRemoteServer creates placeholder
    const placeholder = {
      id: 'placeholder-1',
      baseUrl: 'https://remote.example.com',
      slug: 'remote',
      label: 'Remote Server',
      identityId: '',
      personaId: null,
      accentColor: '#e63946',
      vrpTopic: 'annex:server:remote:v1',
      lastConnectedAt: new Date().toISOString(),
      cachedSummary: { slug: 'remote', label: 'Remote Server', total_active_members: 5 },
    };
    mockCreateServerEntry.mockReturnValueOnce(placeholder);
    mockListServers.mockResolvedValueOnce([placeholder]);

    const result = await useServersStore.getState().addRemoteServer('https://remote.example.com');
    expect(result).not.toBeNull();
    expect(result!.identityId).toBe('');

    // Step 2: After registration, saveCurrentServer should update the placeholder
    useServersStore.setState({ servers: [placeholder] });
    mockListServers.mockResolvedValueOnce([{ ...placeholder, identityId: 'identity-xyz' }]);

    await useServersStore.getState().saveCurrentServer(
      'identity-xyz', 'remote', 'Remote Server', 'https://remote.example.com',
    );

    // Placeholder should have been updated in-place
    expect(mockCreateServerEntry).toHaveBeenCalledTimes(1); // only the initial addRemoteServer call
    expect(mockSaveServer).toHaveBeenCalledWith(
      expect.objectContaining({
        id: 'placeholder-1',
        identityId: 'identity-xyz',
      }),
    );
  });

  it('guards async writes during rapid A→B→A switches', async () => {
    const { useServersStore } = await import('./servers');

    const serverA = {
      id: 'srv-a', baseUrl: 'https://a.example.com', slug: 'a', label: 'A', identityId: 'id-a',
      personaId: null, accentColor: '#e63946', vrpTopic: '', lastConnectedAt: '2025-01-01', cachedSummary: null,
    };
    const serverB = {
      id: 'srv-b', baseUrl: 'https://b.example.com', slug: 'b', label: 'B', identityId: 'id-b',
      personaId: null, accentColor: '#646cff', vrpTopic: '', lastConnectedAt: '2025-01-02', cachedSummary: null,
    };
    useServersStore.setState({ servers: [serverA, serverB], activeServerId: null });

    let resolveBImage: ((value: { image_url: string | null }) => void) | null = null;
    mockGetServerImage.mockImplementation(() => new Promise((resolve) => {
      if (mockCurrentApiBaseUrl === 'https://b.example.com') {
        resolveBImage = resolve;
        return;
      }
      resolve({ image_url: '/a.png' });
    }));
    mockGetServerSummary.mockImplementation(async () => ({ slug: mockCurrentApiBaseUrl, label: mockCurrentApiBaseUrl }));

    await useServersStore.getState().switchServer('srv-a');
    expect(useServersStore.getState().serverImageUrl).toBe('/a.png');

    const pendingB = useServersStore.getState().switchServer('srv-b');
    await Promise.resolve();
    const backToA = useServersStore.getState().switchServer('srv-a');
    await backToA;

    resolveBImage?.({ image_url: '/b.png' });
    await pendingB;

    expect(useServersStore.getState().activeServerId).toBe('srv-a');
    expect(useServersStore.getState().serverImageUrl).toBe('/a.png');
    expect(mockUpdateCachedSummary).not.toHaveBeenCalledWith('srv-b', expect.anything());
  });
});

describe('switchServer rollback is scoped to the switch that owns the context', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  /// A switch that fails after being overtaken must not undo the one that
  /// overtook it.
  ///
  /// Every mutation on the success path is guarded by `switchEpoch === epoch`.
  /// The rollback was not, so a slow failure landed after the user had
  /// started another switch and unconditionally repointed the API base URL,
  /// the identity and the WebSocket at ITS previous server — leaving the user
  /// on a server they did not choose, with the wrong channels listed and no
  /// error that explains it.
  it('a superseded switch does not repoint the app on failure', async () => {
    const { useServersStore } = await import('./servers');

    const server = {
      id: 'srv-slow', baseUrl: 'https://slow.example.com', slug: 'slow', label: 'Slow',
      identityId: 'id-slow', personaId: null, accentColor: '#e63946',
      vrpTopic: '', lastConnectedAt: '2025-01-01', cachedSummary: null,
    };
    useServersStore.setState({
      servers: [server],
      activeServerId: 'srv-original',
      switchEpoch: 0,
    });
    mockListServers.mockResolvedValue([server]);

    // Fail the switch, and while it is failing, let a newer switch bump the
    // epoch — exactly what a second click does.
    mockLoadPermissions.mockImplementationOnce(async () => {
      useServersStore.setState((s) => ({ switchEpoch: s.switchEpoch + 1 }));
      throw new Error('permissions failed');
    });

    mockSetApiBaseUrl.mockClear();
    mockConnectWs.mockClear();

    await expect(useServersStore.getState().switchServer('srv-slow')).rejects.toThrow();

    // The failure is still reported to the caller (the rejection above), but
    // nothing shared was touched on the way out.
    const setBaseUrlCalls = mockSetApiBaseUrl.mock.calls.map((c) => c[0]);
    expect(
      setBaseUrlCalls.filter((u) => u !== 'https://slow.example.com'),
    ).toEqual([]);
    expect(useServersStore.getState().activeServerId).not.toBe('srv-original');
  });

  /// The rollback still has to work when the switch is the current one —
  /// the guard must not disable error recovery.
  it('a current switch still rolls back on failure', async () => {
    const { useServersStore } = await import('./servers');

    const server = {
      id: 'srv-fail', baseUrl: 'https://fail.example.com', slug: 'fail', label: 'Fail',
      identityId: 'id-fail', personaId: null, accentColor: '#e63946',
      vrpTopic: '', lastConnectedAt: '2025-01-01', cachedSummary: null,
    };
    useServersStore.setState({
      servers: [server],
      activeServerId: 'srv-prev',
      switchEpoch: 0,
    });
    mockListServers.mockResolvedValue([server]);
    mockLoadPermissions.mockImplementationOnce(async () => {
      throw new Error('permissions failed');
    });

    await expect(useServersStore.getState().switchServer('srv-fail')).rejects.toThrow();

    expect(useServersStore.getState().activeServerId).toBe('srv-prev');
    expect(useServersStore.getState().switchError).toBeTruthy();
  });
});

describe('a registration that fails says which of the two things failed', () => {
  // `beginRemoteRegistration` returns null for an unreachable server AND for
  // a local identity clone that could not be made, and all five callers
  // rendered their own wording of "could not reach the server". For the
  // second case that is a misdiagnosis: nothing was unreachable, and the
  // user is sent to check a network that is working.
  beforeEach(() => {
    vi.clearAllMocks();
    mockListServers.mockResolvedValue([]);
    mockCloneForServer.mockResolvedValue('cloned-identity-1');
    mockGetRemoteServerSummary.mockResolvedValue({
      slug: 'remote',
      label: 'Remote Server',
      total_active_members: 5,
    });
  });

  it('names the address and the reason when the server cannot be reached', async () => {
    mockGetRemoteServerSummary.mockRejectedValueOnce(new Error('Failed to fetch'));
    const { useServersStore } = await import('./servers');
    useServersStore.setState({ servers: [], registrationError: null });

    const result = await useServersStore.getState().beginRemoteRegistration('https://peer.example');

    expect(result).toBeNull();
    const why = useServersStore.getState().registrationError ?? '';
    expect(why).toContain('https://peer.example');
    expect(why).toContain('Failed to fetch');
  });

  it('does not blame the network when the local identity clone fails', async () => {
    mockCloneForServer.mockResolvedValueOnce(null);
    const { useServersStore } = await import('./servers');
    useServersStore.setState({ servers: [], registrationError: null });

    const result = await useServersStore.getState().beginRemoteRegistration('https://peer.example');

    expect(result).toBeNull();
    const why = useServersStore.getState().registrationError ?? '';
    expect(why).toMatch(/local identity/i);
    expect(why).not.toMatch(/could not reach/i);
    expect(why).toMatch(/existing identity is unchanged/i);
  });
});
