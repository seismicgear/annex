/**
 * App.tsx startup flow tests.
 *
 * These tests verify the Tauri EXE startup flow:
 *   1. Launch → IdentitySetup screen (always first)
 *   2. User selects/creates identity → StartupModeSelector screen
 *   3. User picks host/connect → main app
 *
 * The critical bug: IndexedDB persists across EXE reinstalls on Windows,
 * causing loadIdentities() to auto-select a stored identity and skip
 * IdentitySetup.  The fix calls logout() unconditionally after loading
 * so the user always lands on IdentitySetup first.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, act } from '@testing-library/react';
import { useIdentityStore } from '@/stores/identity';
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

vi.mock('@/lib/zk', () => ({
  initPoseidon: vi.fn(async () => {}),
  generateSecretKey: vi.fn(() => BigInt(42)),
  generateNodeId: vi.fn(() => 'node-1'),
  computeCommitment: vi.fn(async () => '0xabc'),
  generateMembershipProof: vi.fn(async () => ({ proof: {}, publicSignals: [] })),
}));

// ── Mock all child components to isolate App routing logic ──

vi.mock('@/components/IdentitySetup', () => ({
  IdentitySetup: () => <div data-testid="identity-setup">Annex Identity</div>,
}));

vi.mock('@/components/StartupModeSelector', () => ({
  StartupModeSelector: ({ onReady }: { onReady: (url?: string) => void }) => (
    <div data-testid="startup-mode-selector">
      Choose how to use Annex
      <button data-testid="pick-mode-btn" onClick={() => onReady()}>Pick Mode</button>
    </div>
  ),
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

vi.mock('@/lib/startup-prefs', () => ({ clearWebStartupMode: vi.fn() }));
vi.mock('@/lib/invite', () => ({
  parseInviteFromUrl: vi.fn(() => null),
  clearInviteFromUrl: vi.fn(),
}));
vi.mock('@/lib/personas', () => ({ getPersonasForIdentity: vi.fn(async () => []) }));
vi.mock('@/lib/api', () => ({
  getApiBaseUrl: vi.fn(() => 'http://localhost:3000'),
  setApiBaseUrl: vi.fn(),
  setPublicUrl: vi.fn(async () => {}),
  register: vi.fn(async () => ({})),
  verifyMembership: vi.fn(async () => ({})),
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
  startTunnel: vi.fn(async () => 'https://tunnel.example.com'),
  stopTunnel: vi.fn(async () => {}),
  getTunnelUrl: vi.fn(async () => null),
}));

// ── Helpers ──

const FAKE_IDENTITY: StoredIdentity = {
  id: 'test-id-1',
  sk: 'deadbeef',
  roleCode: 1,
  nodeId: 1,
  commitmentHex: '0xabc',
  pseudonymId: 'pseudo-123',
  serverSlug: 'default',
  leafIndex: 0,
  createdAt: '2025-01-01T00:00:00Z',
};

function resetStores() {
  useIdentityStore.setState({
    phase: 'uninitialized',
    identity: null,
    error: null,
    storedIdentities: [],
    permissions: null,
  });
}

beforeEach(() => {
  vi.clearAllMocks();
  tauriEnabled = false;
  resetStores();
});

// ── Tests ──

describe('App startup flow', () => {
  describe('Web mode (not Tauri)', () => {
    it('shows IdentitySetup when no identity exists', async () => {
      const App = (await import('./App')).default;
      render(<App />);

      expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      expect(screen.queryByTestId('startup-mode-selector')).not.toBeInTheDocument();
    });

    it('shows StartupModeSelector when identity is already ready', async () => {
      useIdentityStore.setState({
        phase: 'ready',
        identity: FAKE_IDENTITY,
        storedIdentities: [FAKE_IDENTITY],
      });

      const App = (await import('./App')).default;
      render(<App />);

      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
      expect(screen.getByTestId('startup-mode-selector')).toBeInTheDocument();
    });
  });

  describe('Tauri mode — the critical EXE startup flow', () => {
    beforeEach(() => {
      tauriEnabled = true;
    });

    it('shows loading screen initially while server starts', async () => {
      const App = (await import('./App')).default;
      render(<App />);

      expect(screen.getByText('Starting server...')).toBeInTheDocument();
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
      expect(screen.queryByTestId('startup-mode-selector')).not.toBeInTheDocument();
    });

    it('shows IdentitySetup after server starts with no persisted identity', async () => {
      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.queryByText('Starting server...')).not.toBeInTheDocument();
      });

      expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      expect(screen.queryByTestId('startup-mode-selector')).not.toBeInTheDocument();
    });

    it('shows IdentitySetup even when IndexedDB has a valid persisted identity (THE REINSTALL BUG)', async () => {
      // THIS IS THE CRITICAL TEST.
      //
      // Scenario: user reinstalls the EXE on Windows. The Tauri WebView's
      // IndexedDB persists in %LOCALAPPDATA% across installs. loadIdentities()
      // finds a valid identity and auto-selects it (phase='ready').
      //
      // WITHOUT the fix: app skips IdentitySetup → shows StartupModeSelector.
      // WITH the fix: logout() resets phase → IdentitySetup shows first.

      // Make listIdentities return a ready identity so loadIdentities()
      // auto-selects it (simulating persisted IndexedDB).
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);

      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.queryByText('Starting server...')).not.toBeInTheDocument();
      });

      // IdentitySetup MUST show — this is the whole point of the fix
      expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      expect(screen.queryByTestId('startup-mode-selector')).not.toBeInTheDocument();

      // Store state: phase must be reset, but storedIdentities preserved
      const state = useIdentityStore.getState();
      expect(state.phase).toBe('uninitialized');
      expect(state.storedIdentities).toHaveLength(1);
      expect(state.storedIdentities[0].pseudonymId).toBe('pseudo-123');
    });

    it('after user selects identity on IdentitySetup, shows StartupModeSelector', async () => {
      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      });

      // Simulate user clicking an existing identity
      act(() => {
        useIdentityStore.setState({
          phase: 'ready',
          identity: FAKE_IDENTITY,
        });
      });

      await waitFor(() => {
        expect(screen.getByTestId('startup-mode-selector')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });

    it('full Tauri flow: loading → IdentitySetup → StartupModeSelector → main app', async () => {
      const App = (await import('./App')).default;
      render(<App />);

      // Step 1: Loading
      expect(screen.getByText('Starting server...')).toBeInTheDocument();

      // Step 2: IdentitySetup
      await waitFor(() => {
        expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      });

      // Step 3: User selects identity
      act(() => {
        useIdentityStore.setState({
          phase: 'ready',
          identity: FAKE_IDENTITY,
          storedIdentities: [FAKE_IDENTITY],
        });
      });

      // Step 4: StartupModeSelector
      await waitFor(() => {
        expect(screen.getByTestId('startup-mode-selector')).toBeInTheDocument();
      });

      // Step 5: User picks a mode
      await act(async () => {
        screen.getByTestId('pick-mode-btn').click();
      });

      // Step 6: Main app
      await waitFor(() => {
        expect(screen.getByTestId('status-bar')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
      expect(screen.queryByTestId('startup-mode-selector')).not.toBeInTheDocument();
    });

    it('shows error screen with retry when embedded server fails to start', async () => {
      const tauri = await import('@/lib/tauri');
      vi.mocked(tauri.startEmbeddedServer).mockRejectedValueOnce(
        new Error('Port 9999 already in use'),
      );

      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByText('Port 9999 already in use')).toBeInTheDocument();
      });

      expect(screen.getByText('Retry')).toBeInTheDocument();
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
      expect(screen.queryByTestId('startup-mode-selector')).not.toBeInTheDocument();
    });
  });

  describe('Render order invariants', () => {
    it('IdentitySetup renders when phase is uninitialized (web mode)', async () => {
      tauriEnabled = false;
      resetStores();
      const App = (await import('./App')).default;
      render(<App />);

      expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
    });

    it('IdentitySetup renders when phase is ready but pseudonymId is null', async () => {
      tauriEnabled = false;
      useIdentityStore.setState({
        phase: 'ready',
        identity: { ...FAKE_IDENTITY, pseudonymId: null },
      });

      const App = (await import('./App')).default;
      render(<App />);

      expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      expect(screen.queryByTestId('startup-mode-selector')).not.toBeInTheDocument();
    });

    it('StartupModeSelector renders only when phase is ready AND pseudonymId exists', async () => {
      tauriEnabled = false;
      useIdentityStore.setState({
        phase: 'ready',
        identity: FAKE_IDENTITY,
        storedIdentities: [FAKE_IDENTITY],
      });

      const App = (await import('./App')).default;
      render(<App />);

      expect(screen.getByTestId('startup-mode-selector')).toBeInTheDocument();
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });
  });
});
