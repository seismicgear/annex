import { describe, it, expect, vi, beforeEach } from 'vitest';

// Mock dependencies before importing the store
vi.mock('@/lib/servers', () => ({
  listServers: vi.fn(async () => []),
  saveServer: vi.fn(async () => {}),
  getServerByIdentityId: vi.fn(async () => null),
  createServerEntry: vi.fn(() => ({ id: 'test-id', baseUrl: '', identityId: '' })),
  removeServer: vi.fn(async () => {}),
  updateCachedSummary: vi.fn(async () => {}),
}));

const mockGetServerImage = vi.fn(async () => ({ image_url: null }));
vi.mock('@/lib/api', () => ({
  setApiBaseUrl: vi.fn(),
  getServerImage: (...args: unknown[]) => mockGetServerImage(...args),
  getServerSummary: vi.fn(async () => ({})),
  resolveUrl: (url: string) => url,
}));

vi.mock('./identity', () => ({
  useIdentityStore: {
    getState: () => ({
      identity: { pseudonymId: 'p1' },
      selectIdentity: vi.fn(async () => {}),
      loadPermissions: vi.fn(async () => {}),
    }),
  },
}));

vi.mock('./channels', () => ({
  useChannelsStore: {
    getState: () => ({
      disconnectWs: vi.fn(),
      connectWs: vi.fn(),
      loadChannels: vi.fn(async () => {}),
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
    vi.resetModules();
  });

  it('fetchServerImage sets serverImageUrl to null on failure', async () => {
    const { useServersStore } = await import('./servers');
    mockGetServerImage.mockRejectedValueOnce(new Error('404'));

    // Set a stale image first
    useServersStore.getState().setServerImageUrl('https://old-server/image.png');
    expect(useServersStore.getState().serverImageUrl).toBe('https://old-server/image.png');

    // fetchServerImage should clear it on error
    await useServersStore.getState().fetchServerImage();
    expect(useServersStore.getState().serverImageUrl).toBeNull();
  });

  it('fetchServerImage sets null when server has no image', async () => {
    const { useServersStore } = await import('./servers');
    mockGetServerImage.mockResolvedValueOnce({ image_url: null });

    useServersStore.getState().setServerImageUrl('https://old/img.png');
    await useServersStore.getState().fetchServerImage();
    expect(useServersStore.getState().serverImageUrl).toBeNull();
  });
});
