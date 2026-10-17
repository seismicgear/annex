/**
 * App.tsx startup flow tests.
 *
 * The startup flow has two sequential screens with a hard gate:
 *
 *   Screen 1 — Identity creation (offline, zero network requests, no server)
 *     Shows when no identity keys exist in local storage.
 *     No server is started, contacted, or registered with.
 *
 *   Screen 2 — Server / startup-mode selection
 *     Shows ONLY after identity keys exist.
 *     The server is started here (not before) if the user picks "Host".
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, act } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useIdentityStore } from '@/stores/identity';
import { useServersStore } from '@/stores/servers';
import { useChannelsStore } from '@/stores/channels';
import type { StoredIdentity } from '@/types';

// ── Mock IndexedDB-backed modules (jsdom has no IndexedDB) ──

vi.mock('@/lib/db', () => ({
  listIdentities: vi.fn(async () => []),
  saveIdentity: vi.fn(async () => {}),
  getIdentity: vi.fn(async () => undefined),
  deleteIdentity: vi.fn(async () => {}),
  updateIdentityPseudonym: vi.fn(async () => {}),
  exportIdentity: vi.fn(() => '{}'),
  importIdentity: vi.fn(async () => ({})),
  clearAllDatabases: vi.fn(async () => {}),
}));

vi.mock('@/lib/servers', () => ({
  listServers: vi.fn(async () => []),
  saveServer: vi.fn(async () => {}),
  getServer: vi.fn(async () => undefined),
  getServerByIdentityId: vi.fn(async () => undefined),
  getServerBySlug: vi.fn(async () => undefined),
  removeServer: vi.fn(async () => {}),
  updateCachedSummary: vi.fn(async () => {}),
  createServerEntry: vi.fn(() => ({})),
  randomAccentColor: vi.fn(() => '#e63946'),
}));

vi.mock('@/lib/zk', () => {
  class ZkProofAssetsError extends Error {
    constructor(message: string) {
      super(message);
      this.name = 'ZkProofAssetsError';
    }
  }
  class ZkProofTimeoutError extends Error {
    constructor(message: string) {
      super(message);
      this.name = 'ZkProofTimeoutError';
    }
  }
  class ZkProofInFlightError extends Error {
    constructor(message: string) {
      super(message);
      this.name = 'ZkProofInFlightError';
    }
  }
  class ZkProofCancelledError extends Error {
    constructor(message: string) {
      super(message);
      this.name = 'ZkProofCancelledError';
    }
  }
  return {
    initPoseidon: vi.fn(async () => {}),
    generateSecretKey: vi.fn(() => BigInt(42)),
    generateNodeId: vi.fn(() => 1),
    computeCommitment: vi.fn(async () => '0xabc'),
    generateMembershipProof: vi.fn(async () => ({ proof: {}, publicSignals: [] })),
    generateMembershipProofV2: vi.fn(async () => ({ proof: {}, publicSignals: [], nullifierHex: 'n', topicHashHex: 't', challengeHex: 'c' })),
    cancelMembershipProofGeneration: vi.fn(async () => {}),
    isProofGenerationInFlight: vi.fn(() => false),
    ZkProofAssetsError,
    ZkProofTimeoutError,
    ZkProofInFlightError,
    ZkProofCancelledError,
  };
});

// ── Mock all child components to isolate App routing logic ──

vi.mock('@/components/IdentitySetup', () => ({
  IdentitySetup: () => <div data-testid="identity-setup">Create Your Identity</div>,
}));

vi.mock('@/components/ChannelList', () => ({ ChannelList: () => <div data-testid="channel-list" /> }));
vi.mock('@/components/MessageView', () => ({ MessageView: () => <div data-testid="message-view" /> }));
vi.mock('@/components/MessageInput', () => ({ MessageInput: () => <div data-testid="message-input" /> }));
vi.mock('@/components/VoicePanel', () => ({ VoicePanel: () => <div data-testid="voice-panel" /> }));
vi.mock('@/components/MemberList', () => ({ MemberList: () => <div data-testid="member-list" /> }));
vi.mock('@/components/StatusBar', () => ({ StatusBar: () => <div data-testid="status-bar" /> }));
vi.mock('@/components/FederationPanel', () => ({ FederationPanel: () => <div data-testid="federation-panel" /> }));
vi.mock('@/components/EventLog', () => ({ EventLog: () => <div data-testid="event-log" /> }));
vi.mock('@/components/AdminPanel', () => ({ AdminPanel: () => <div data-testid="admin-panel" /> }));
vi.mock('@/components/ServerHub', () => ({ ServerHub: () => <div data-testid="server-hub" /> }));
vi.mock('@/components/DeviceLinkDialog', () => ({ DeviceLinkDialog: () => null }));

vi.mock('@/lib/startup-prefs', () => ({ clearWebStartupMode: vi.fn(), saveWebStartupMode: vi.fn() }));
vi.mock('@/lib/invite', () => ({
  parseLegacyInviteFromUrl: vi.fn(() => null),
  clearInviteFromUrl: vi.fn(),
}));
vi.mock('@/lib/personas', () => ({ getPersonasForIdentity: vi.fn(async () => []) }));
vi.mock('@/lib/api', () => ({
  getApiBaseUrl: vi.fn(() => 'http://localhost:3000'),
  setApiBaseUrl: vi.fn(),
  setPublicUrl: vi.fn(async () => ({ status: 'ok', server_slug: 'default' })),
  setWebrtcPublicUrl: vi.fn(async () => ({ status: 'ok' })),
  createInvite: vi.fn(async () => ({ code: 'created-invite', url: 'https://monolithannex.com/invite/test' })),
  setSessionToken: vi.fn(),
  setZkProofPayload: vi.fn(),
  // Credentials are bound to a (server, identity) context; anything that
  // outlives an await re-checks it before touching identity-scoped state.
  // `useSessionConnection` does, so the mock has to supply it.
  getCredentialContext: vi.fn(() => ({ serial: 0, baseUrl: 'http://localhost:3000', pseudonymId: 'pseudo-123' })),
  isCredentialContextCurrent: vi.fn(() => true),
  StaleCredentialContextError: class StaleCredentialContextError extends Error {},
  getCurrentRoot: vi.fn(async () => ({ rootHex: 'ROOT', leafCount: 1 })),
  getSessionToken: vi.fn(() => null),
  isTokenExpired: vi.fn(() => false),
  startTokenRefresh: vi.fn(),
  stopTokenRefresh: vi.fn(),
  register: vi.fn(async () => ({
    identityId: 1,
    leafIndex: 0,
    rootHex: '0x123',
    pathElements: ['0x1'],
    pathIndexBits: [0],
  })),
  requestAuthChallenge: vi.fn(async () => ({ challenge: 'c'.repeat(64), expiresInSecs: 300 })),
  verifyMembership: vi.fn(async () => ({
    ok: true,
    pseudonymId: 'pseudo-123',
    sessionToken: 'test-session-token',
  })),
  getIdentityInfo: vi.fn(async () => ({
    pseudonymId: 'pseudo-123',
    participantType: 'human',
    active: true,
    capabilities: {
      can_voice: false,
      can_moderate: false,
      can_invite: false,
      can_federate: false,
      can_bridge: false,
    },
  })),
  getServerSummary: vi.fn(async () => ({
    slug: 'default',
    label: 'Default',
    members_by_type: {},
    total_active_members: 0,
    channel_count: 0,
    federation_peer_count: 0,
    active_agent_count: 0,
  })),
  redeemInvite: vi.fn(async () => ({ valid: true, serverSlug: 'invited-server' })),
  getMessages: vi.fn(async () => []),
  getChannels: vi.fn(async () => []),
  getServerImage: vi.fn(async () => null),
}));
vi.mock('@/lib/ws', () => ({
  AnnexWebSocket: class MockWebSocket {
    disconnect = vi.fn();
    connect = vi.fn();
    send = vi.fn();
    onMessage = vi.fn();
    onStatus = vi.fn();
    subscribe = vi.fn();
    unsubscribe = vi.fn();
  },
}));
vi.mock('./App.css', () => ({}));

// ── Mock isTauri — controlled per test ──

let tauriEnabled = false;

vi.mock('@/lib/tauri', () => ({
  isTauri: () => tauriEnabled,
  startEmbeddedServer: vi.fn(async () => 'http://127.0.0.1:9999'),
  getStartupMode: vi.fn(async () => null),
  saveStartupMode: vi.fn(async () => {}),
  clearStartupMode: vi.fn(async () => {}),
  getHostIdentityContext: vi.fn(async () => ({ server_slug: 'default', founder_pseudonym_id: null as string | null })),
  acquirePublicEndpoint: vi.fn(async () => 'https://host-abc123.router.annex.net'),
  releasePublicEndpoint: vi.fn(async () => {}),
  getPublicEndpoint: vi.fn(async () => null),
  getWebRtcConfig: vi.fn(async () => ({ configured: false, url: '', api_key: '', has_api_secret: false, token_ttl_seconds: 3600 })),
  startLocalWebRtc: vi.fn(async () => ({ url: 'ws://127.0.0.1:7880' })),
  checkWebRtcReachable: vi.fn(async () => ({ reachable: true })),
  exportIdentityJson: vi.fn(async () => null),
  getPlatformMediaStatus: vi.fn(async () => ({ screen_share_available: true, camera_mic_available: true, warnings: [], display_server: 'test' })),
  listenForInvite: vi.fn(async () => () => {}),
  getPendingInvite: vi.fn(async () => null),
  ackPendingInvite: vi.fn(async () => {}),
  checkFirstRunCompleted: vi.fn(async () => false),
  markFirstRunCompleted: vi.fn(async () => {}),
  resetServerData: vi.fn(async () => {}),
  clearWebRtcEnv: vi.fn(async () => {}),
}));

// ── Helpers ──

const FAKE_IDENTITY: StoredIdentity = {
  id: 'test-id-1',
  sk: 'deadbeef',
  roleCode: 1,
  nodeId: 1,
  commitmentHex: '0xabc',
  pseudonymId: 'pseudo-123',
  sessionToken: 'test-session-token',
  serverSlug: 'default',
  leafIndex: 0,
  zkProofPayload: JSON.stringify({ root_hex: 'ROOT' }),
  createdAt: '2025-01-01T00:00:00Z',
};

/** Identity that has keys but is not yet registered with any server. */
const KEYS_ONLY_IDENTITY: StoredIdentity = {
  ...FAKE_IDENTITY,
  pseudonymId: null,
  serverSlug: '',
  leafIndex: null,
};

function resetStores() {
  useIdentityStore.setState({
    phase: 'uninitialized',
    identity: null,
    error: null,
    storedIdentities: [],
    permissions: null,
  });
  useServersStore.setState({
    servers: [],
    activeServerId: null,
    serverImageUrl: null,
    pendingRegistrationServerId: null,
    switchError: null,
  });
  useChannelsStore.setState({
    wsConnected: false,
    ws: null,
  });
}

beforeEach(async () => {
  vi.clearAllMocks();
  tauriEnabled = false;
  resetStores();
  // Restore default mock: no identities in IndexedDB.
  // Tests that need pre-existing identities must override this BEFORE render.
  const dbMock = await import('@/lib/db');
  vi.mocked(dbMock.listIdentities).mockResolvedValue([]);
  const stored = new Map<string, StoredIdentity>();
  vi.mocked(dbMock.saveIdentity).mockImplementation(async identity => { stored.set(identity.id, { ...identity }); });
  vi.mocked(dbMock.getIdentity).mockImplementation(async id => stored.get(id) ?? (await dbMock.listIdentities()).find(i => i.id === id));
  // Restore default Tauri mocks — vi.clearAllMocks() only clears call counts,
  // it does NOT reset implementations set by mockResolvedValue in earlier tests.
  const tauriMock = await import('@/lib/tauri');
  vi.mocked(tauriMock.getStartupMode).mockResolvedValue(null);
  vi.mocked(tauriMock.getHostIdentityContext).mockResolvedValue({ server_slug: 'default', founder_pseudonym_id: null });
  vi.mocked(tauriMock.startEmbeddedServer).mockResolvedValue('http://127.0.0.1:9999');
  vi.mocked(tauriMock.checkFirstRunCompleted).mockResolvedValue(false);
  vi.mocked(tauriMock.getPendingInvite).mockResolvedValue(null);
  global.fetch = vi.fn(async () => ({ ok: true, status: 200 } as Response));
});

// ── Tests ──

describe('App startup flow', () => {
  describe('ReconnectionBanner', () => {
    it('keeps a displaced session paused across grace timers and only reports explicit recovery', async () => {
      vi.useFakeTimers();
      useChannelsStore.setState({ wsConnected: false, wsSessionState: 'taken_over' });
      const { ReconnectionBanner } = await import('./App');
      render(<ReconnectionBanner />);
      act(() => { vi.advanceTimersByTime(60_000); });
      expect(screen.getByRole('alert')).toHaveTextContent('Live session paused. Use Reconnect here');
      expect(screen.queryByText('Connection lost — reconnecting...')).not.toBeInTheDocument();
      act(() => { useChannelsStore.setState({ wsConnected: true, wsSessionState: 'connected' }); });
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      act(() => { useChannelsStore.setState({ wsConnected: false, wsSessionState: 'taken_over' }); });
      expect(screen.getByRole('alert')).toHaveTextContent('Live session paused. Use Reconnect here');
      useChannelsStore.setState({ wsSessionState: 'disconnected' });
      vi.useRealTimers();
    });

    it('stays silent while the first connection is still being established', async () => {
      // `wsConnected` is false on every load simply because the socket has
      // not opened yet. Announcing that as a lost connection meant every
      // single page load flashed "Connection lost" then "Reconnected".
      useChannelsStore.setState({ wsConnected: false });
      const { ReconnectionBanner } = await import('./App');
      render(<ReconnectionBanner />);

      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
    });

    it('says "connecting" — not "lost" — if the first connection is slow', async () => {
      vi.useFakeTimers();
      useChannelsStore.setState({ wsConnected: false });
      const { ReconnectionBanner } = await import('./App');
      render(<ReconnectionBanner />);

      act(() => {
        vi.advanceTimersByTime(4000);
      });

      expect(screen.getByRole('alert')).toHaveTextContent('Connecting to server...');
      vi.useRealTimers();
    });

    it('does not claim a reconnection when the first connection succeeds', async () => {
      vi.useFakeTimers();
      useChannelsStore.setState({ wsConnected: false });
      const { ReconnectionBanner } = await import('./App');
      render(<ReconnectionBanner />);

      act(() => {
        useChannelsStore.setState({ wsConnected: true });
      });

      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      vi.useRealTimers();
    });

    it('reports a genuine drop and recovery, auto-hiding after 2 seconds', async () => {
      vi.useFakeTimers();
      // Start connected: this session has something to lose.
      useChannelsStore.setState({ wsConnected: true });
      const { ReconnectionBanner } = await import('./App');
      render(<ReconnectionBanner />);
      expect(screen.queryByRole('alert')).not.toBeInTheDocument();

      act(() => {
        useChannelsStore.setState({ wsConnected: false });
      });

      expect(screen.getByRole('alert')).toHaveTextContent('Connection lost — reconnecting...');

      act(() => {
        useChannelsStore.setState({ wsConnected: true });
      });

      expect(screen.getByRole('alert')).toHaveTextContent('Reconnected');

      act(() => {
        vi.advanceTimersByTime(2000);
      });

      expect(screen.queryByRole('alert')).not.toBeInTheDocument();
      vi.useRealTimers();
    });
  });

  describe('Web mode (not Tauri)', () => {
    it('shows IdentitySetup when no identity exists', async () => {
      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      });
      expect(screen.queryByText('Choose how to use Annex. Remembered values are shown as suggestions.')).not.toBeInTheDocument();
    });

    it('shows StartupModeSelector when identity with pseudonymId exists', async () => {
      // Mock IndexedDB to contain a fully registered identity so
      // loadIdentities() auto-selects it (phase='ready').
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);

      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });

    it('shows StartupModeSelector (not IdentitySetup) when identity has keys but no pseudonymId', async () => {
      // Keys exist → skip Screen 1 → Screen 2
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([KEYS_ONLY_IDENTITY]);

      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });
  });

  describe('Tauri mode — explicit one-click Host setup', () => {
    beforeEach(() => { tauriEnabled = true; });

    it('offers Host immediately without starting a server or generating keys before the click', async () => {
      const App = (await import('./App')).default;
      const tauri = await import('@/lib/tauri');
      const zk = await import('@/lib/zk');
      render(<App />);
      expect(await screen.findByRole('button', { name: 'Start Hosting' })).toBeInTheDocument();
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
      expect(tauri.startEmbeddedServer).not.toHaveBeenCalled();
      expect(zk.generateSecretKey).not.toHaveBeenCalled();
    });

    it('keeps explicit identity import available without starting a host', async () => {
      const App = (await import('./App')).default;
      const tauri = await import('@/lib/tauri');
      render(<App />);
      await userEvent.click(await screen.findByRole('button', { name: 'Restore an existing identity' }));
      expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      expect(tauri.startEmbeddedServer).not.toHaveBeenCalled();
    });

    it('one Host click creates keys, registers and verifies them before showing the main app', async () => {
      const App = (await import('./App')).default;
      const api = await import('@/lib/api');
      const tauri = await import('@/lib/tauri');
      render(<App />);
      await userEvent.click(await screen.findByRole('button', { name: 'Start Hosting' }));
      await screen.findByTestId('status-bar');
      expect(tauri.startEmbeddedServer).toHaveBeenCalledTimes(1);
      expect(api.register).toHaveBeenCalledTimes(1);
      expect(api.requestAuthChallenge).toHaveBeenCalledTimes(1);
      expect(api.verifyMembership).toHaveBeenCalledTimes(1);
      expect(useIdentityStore.getState().identity?.pseudonymId).toBe('pseudo-123');
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });

    it('restores founder permissions and the session only after the saved host is ready', async () => {
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);
      const tauri = await import('@/lib/tauri');
      const api = await import('@/lib/api');
      let finishEndpoint!: (url: string) => void;
      vi.mocked(tauri.acquirePublicEndpoint).mockReturnValueOnce(new Promise(resolve => { finishEndpoint = resolve; }));
      const connect = vi.spyOn(useChannelsStore.getState(), 'connectWs');
      const saveCurrentServer = vi.spyOn(useServersStore.getState(), 'saveCurrentServer');
      vi.mocked(tauri.getStartupMode).mockResolvedValue({ startup_mode: { mode: 'host' } });
      vi.mocked(tauri.getHostIdentityContext).mockResolvedValue({ server_slug: 'default', founder_pseudonym_id: 'pseudo-123' });
      vi.mocked(tauri.checkFirstRunCompleted).mockResolvedValue(true);
      const App = (await import('./App')).default;
      render(<App />);
      await waitFor(() => expect(tauri.acquirePublicEndpoint).toHaveBeenCalled());
      expect(api.getIdentityInfo).not.toHaveBeenCalled();
      expect(connect).not.toHaveBeenCalled();
      expect(saveCurrentServer).not.toHaveBeenCalled();
      await act(async () => { finishEndpoint('https://www.monolithannex.com/host/' + 'a'.repeat(64)); });
      await screen.findByTestId('status-bar');
      await waitFor(() => expect(useIdentityStore.getState().permissionsStatus).toBe('ready'));
      expect(api.getIdentityInfo).toHaveBeenCalledWith(FAKE_IDENTITY.pseudonymId);
      expect(connect).toHaveBeenCalledTimes(1);
      expect(useIdentityStore.getState().identity?.id).toBe(FAKE_IDENTITY.id);
      expect(tauri.startEmbeddedServer).toHaveBeenCalledTimes(1);
      connect.mockRestore();
      saveCurrentServer.mockRestore();
    });
  });

  describe('Render order invariants', () => {
    it('IdentitySetup renders when phase is uninitialized and no identity (web mode)', async () => {
      tauriEnabled = false;
      resetStores();
      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      });
    });

    it('StartupModeSelector renders when identity has keys but no pseudonymId', async () => {
      tauriEnabled = false;
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([KEYS_ONLY_IDENTITY]);

      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });

    it('StartupModeSelector renders when phase is ready AND pseudonymId exists', async () => {
      tauriEnabled = false;
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);

      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });

    it('keys-ready identity with saved client prefs auto-resumes connection attempt', async () => {
      tauriEnabled = true;
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([KEYS_ONLY_IDENTITY]);
      const tauri = await import('@/lib/tauri');
      vi.mocked(tauri.getStartupMode).mockResolvedValue({
        startup_mode: { mode: 'client', server_url: 'https://unreachable.invalid' },
      });
      vi.mocked(tauri.checkFirstRunCompleted).mockResolvedValue(true);
      // Make the server health check fail so auto-resume falls back to selector
      global.fetch = vi.fn(async () => { throw new Error('network'); });
      const App = (await import('./App')).default;
      render(<App />);

      // Auto-resume tries to connect, shows "Connecting..." then falls back
      // to selector on failure
      await waitFor(() => {
        expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
      });

      // Selector should not show identity setup
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });

    it('Gate 3 retry returns to StartupModeSelector without immediate re-registration loop', async () => {
      // Use web mode — Gate 3 retry behavior is mode-independent and web
      // mode avoids Tauri getStartupMode async complexities.
      tauriEnabled = false;
      const user = userEvent.setup();
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([KEYS_ONLY_IDENTITY]);
      const api = await import('@/lib/api');
      // Make register() fail so registration cannot complete
      vi.mocked(api.register).mockRejectedValue(new Error('offline'));

      const App = (await import('./App')).default;
      render(<App />);

      // Selector appears (web mode, keys exist but no pseudonymId)
      await waitFor(() => {
        expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
      });

      // Click "Continue" (web mode: "Use This Server") → onReady → serverReady=true
      await user.click(screen.getByRole('button', { name: 'Continue' }));

      // Registration fails (register mock rejects) → error state.
      // Simulate the final error from the retry loop exhaustion to avoid
      // waiting for the real 15.5s exponential backoff.
      act(() => {
        useIdentityStore.setState({
          phase: 'error',
          error: 'Unable to contact server',
        });
      });

      await waitFor(() => {
        expect(screen.getByText(/Unable to contact server/)).toBeInTheDocument();
      });

      const callsBefore = vi.mocked(api.getServerSummary).mock.calls.length;
      await user.click(screen.getByRole('button', { name: 'Retry' }));

      // Retry resets serverReady=false → Gate 2 shows selector again
      await waitFor(() => {
        expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
      });

      // Retry should NOT immediately re-trigger registration
      expect(api.getServerSummary).toHaveBeenCalledTimes(callsBefore);
    });
  });

  describe('Degraded startup and retry flow', () => {
    it('Gate 3 retry clears startup prefs so chooser shows', async () => {
      // Use web mode to avoid Tauri auto-resume complexities
      tauriEnabled = false;
      const user = userEvent.setup();
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([KEYS_ONLY_IDENTITY]);
      const api = await import('@/lib/api');
      vi.mocked(api.register).mockRejectedValue(new Error('offline'));

      const App = (await import('./App')).default;
      render(<App />);

      // Selector appears (web mode, keys exist but no pseudonymId)
      await waitFor(() => {
        expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
      });

      // Click "Continue" → onReady → serverReady=true
      await user.click(screen.getByRole('button', { name: 'Continue' }));

      // Simulate error from registration failure
      act(() => {
        useIdentityStore.setState({
          phase: 'error',
          error: 'Unable to contact server',
        });
      });

      await waitFor(() => {
        expect(screen.getByText(/Unable to contact server/)).toBeInTheDocument();
      });

      // Click retry — should reset and show selector again
      await user.click(screen.getByRole('button', { name: 'Retry' }));

      // Should show startup selector, not re-enter registration
      await waitFor(() => {
        expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
      });
    });

    it('renders degraded startup banner when webrtcRouteUnavailable after host start', async () => {
      tauriEnabled = true;
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);
      const tauri = await import('@/lib/tauri');
      vi.mocked(tauri.getStartupMode).mockResolvedValue({
        startup_mode: { mode: 'host' },
      });
      vi.mocked(tauri.checkFirstRunCompleted).mockResolvedValue(true);
      vi.mocked(tauri.getHostIdentityContext).mockResolvedValue({ server_slug: 'default', founder_pseudonym_id: 'pseudo-123' });
      // getPublicEndpoint returns public_url but no public_webrtc_url
      vi.mocked(tauri.getPublicEndpoint).mockResolvedValue({
        public_url: 'https://host.router.annex.net',
        public_webrtc_url: null,
      } as { public_url: string; public_webrtc_url: string | null });

      const App = (await import('./App')).default;
      render(<App />);

      // Wait for the main app to render
      await waitFor(() => {
        expect(screen.getByTestId('status-bar')).toBeInTheDocument();
      });

      // The degraded banner may be set by the getPublicEndpoint effect
      // We need to wait for the effect to run
      await waitFor(() => {
        expect(screen.getByText(/remote voice\/video is unavailable/)).toBeInTheDocument();
      });
    });
  });

  describe('Fresh install gating — first_run_completed marker', () => {
    it('Tauri: first_run_completed=true skips destructive cleanup on relaunch', async () => {
      tauriEnabled = true;
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);
      const tauri = await import('@/lib/tauri');
      // first_run_completed exists — this is a returning user after logout
      vi.mocked(tauri.checkFirstRunCompleted).mockResolvedValue(true);

      const App = (await import('./App')).default;
      render(<App />);

      // Should NOT have called clearAllDatabases or resetServerData
      await waitFor(() => {
        expect(screen.getByText('Choose how to use Annex. Remembered values are shown as suggestions.')).toBeInTheDocument();
      });
      expect(dbMock.clearAllDatabases).not.toHaveBeenCalled();
    });

    it('Tauri: a missing marker does NOT wipe data when identities exist', async () => {
      // The marker is written by `markFirstRunCompleted()`, whose failure was
      // swallowed at its call site. One failed write — a full disk, a
      // permissions problem, an IPC hiccup — and the marker never appears, so
      // the next launch treated an established install as fresh and deleted
      // every identity, server, persona, message and upload. Silently, and
      // with no way back.
      //
      // Existing identities are direct evidence that this is not a fresh
      // install, and they outrank an absent marker file.
      tauriEnabled = true;
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);
      const tauri = await import('@/lib/tauri');
      vi.mocked(tauri.checkFirstRunCompleted).mockResolvedValue(false);

      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(dbMock.listIdentities).toHaveBeenCalled();
      });
      await waitFor(() => {
        expect(dbMock.clearAllDatabases).not.toHaveBeenCalled();
      });
      expect(vi.mocked(tauri.resetServerData)).not.toHaveBeenCalled();
    });

    it('Tauri: missing browser keys and marker never erase a potentially existing server', async () => {
      tauriEnabled = true;
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([]);
      const tauri = await import('@/lib/tauri');
      vi.mocked(tauri.checkFirstRunCompleted).mockResolvedValue(false);

      const App = (await import('./App')).default;
      render(<App />);

      await screen.findByRole('button', { name: 'Start Hosting' });
      expect(dbMock.clearAllDatabases).not.toHaveBeenCalled();
      expect(tauri.resetServerData).not.toHaveBeenCalled();
    });
  });

  describe('Protocol invite confirmation gate', () => {
    beforeEach(() => {
      tauriEnabled = true;
    });

    it('invite received while active does not auto-switch server', async () => {
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);
      const tauri = await import('@/lib/tauri');
      vi.mocked(tauri.checkFirstRunCompleted).mockResolvedValue(true);
      vi.mocked(tauri.getStartupMode).mockResolvedValue({ startup_mode: { mode: 'host' } });
      vi.mocked(tauri.getPendingInvite).mockResolvedValue(null);
      vi.mocked(tauri.listenForInvite).mockImplementation(async (cb) => {
        setTimeout(() => {
          const delivery = { server: 'https://remote.annex.test', code: 'abc123', deliveryId: 'delivery-1' };
          vi.mocked(tauri.getPendingInvite).mockResolvedValue(delivery);
          cb(delivery);
        }, 0);
        return () => {};
      });
      useServersStore.setState({
        servers: [{ id: 's1', identityId: 'test-id-1', label: 'Local', baseUrl: 'http://127.0.0.1:9999', createdAt: '', updatedAt: '', accentColor: '#fff' }],
        activeServerId: 's1',
      } as never);
      const api = await import('@/lib/api');
      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => expect(screen.getByRole('button', { name: 'Join invite server' })).toBeInTheDocument());
      expect(api.redeemInvite).not.toHaveBeenCalled();
    });

    it('accept path triggers registration flow', async () => {
      const user = userEvent.setup();
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);
      const tauri = await import('@/lib/tauri');
      vi.mocked(tauri.checkFirstRunCompleted).mockResolvedValue(true);
      vi.mocked(tauri.getStartupMode).mockResolvedValue({ startup_mode: { mode: 'host' } });
      vi.mocked(tauri.getPendingInvite).mockResolvedValue(null);
      vi.mocked(tauri.listenForInvite).mockImplementation(async (cb) => {
        setTimeout(() => {
          const delivery = { server: 'https://remote.annex.test', code: 'abc123', deliveryId: 'delivery-1' };
          vi.mocked(tauri.getPendingInvite).mockResolvedValue(delivery);
          cb(delivery);
        }, 0);
        return () => {};
      });
      const beginRemoteRegistration = vi.fn(async () => ({ id: 'remote' }));
      useServersStore.setState({
        servers: [{ id: 's1', identityId: 'test-id-1', label: 'Local', baseUrl: 'http://127.0.0.1:9999', createdAt: '', updatedAt: '', accentColor: '#fff' }],
        activeServerId: 's1',
        beginRemoteRegistration,
      } as never);
      const api = await import('@/lib/api');
      const App = (await import('./App')).default;
      render(<App />);
      await waitFor(() => expect(screen.getByRole('button', { name: 'Join invite server' })).toBeInTheDocument());
      await user.click(screen.getByRole('button', { name: 'Join invite server' }));
      await waitFor(() => expect(api.redeemInvite).toHaveBeenCalledWith('https://remote.annex.test', 'abc123'));
      expect(beginRemoteRegistration).toHaveBeenCalledWith('https://remote.annex.test');
    });

    it('ignore path clears pending invite safely', async () => {
      const user = userEvent.setup();
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);
      const tauri = await import('@/lib/tauri');
      vi.mocked(tauri.checkFirstRunCompleted).mockResolvedValue(true);
      vi.mocked(tauri.getStartupMode).mockResolvedValue({ startup_mode: { mode: 'host' } });
      vi.mocked(tauri.getPendingInvite).mockResolvedValue(null);
      vi.mocked(tauri.listenForInvite).mockImplementation(async (cb) => {
        setTimeout(() => {
          const delivery = { server: 'https://remote.annex.test', code: 'abc123', deliveryId: 'delivery-1' };
          vi.mocked(tauri.getPendingInvite).mockResolvedValue(delivery);
          cb(delivery);
        }, 0);
        return () => {};
      });
      useServersStore.setState({
        servers: [{ id: 's1', identityId: 'test-id-1', label: 'Local', baseUrl: 'http://127.0.0.1:9999', createdAt: '', updatedAt: '', accentColor: '#fff' }],
        activeServerId: 's1',
      } as never);
      const api = await import('@/lib/api');
      const App = (await import('./App')).default;
      render(<App />);
      await waitFor(() => expect(screen.getByRole('button', { name: 'Ignore' })).toBeInTheDocument());
      await user.click(screen.getByRole('button', { name: 'Ignore' }));
      await waitFor(() => expect(screen.queryByRole('button', { name: 'Join invite server' })).not.toBeInTheDocument());
      expect(api.redeemInvite).not.toHaveBeenCalled();
    });
  });
});
