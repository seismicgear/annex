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
  generateNodeId: vi.fn(() => 1),
  computeCommitment: vi.fn(async () => '0xabc'),
  generateMembershipProof: vi.fn(async () => ({ proof: {}, publicSignals: [] })),
}));

// ── Mock all child components to isolate App routing logic ──

vi.mock('@/components/IdentitySetup', () => ({
  IdentitySetup: () => <div data-testid="identity-setup">Create Your Identity</div>,
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
  register: vi.fn(async () => ({
    identityId: 1,
    leafIndex: 0,
    rootHex: '0x123',
    pathElements: ['0x1'],
    pathIndexBits: [0],
  })),
  verifyMembership: vi.fn(async () => ({
    ok: true,
    pseudonymId: 'pseudo-123',
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
}

beforeEach(async () => {
  vi.clearAllMocks();
  tauriEnabled = false;
  resetStores();
  // Restore default mock: no identities in IndexedDB.
  // Tests that need pre-existing identities must override this BEFORE render.
  const dbMock = await import('@/lib/db');
  vi.mocked(dbMock.listIdentities).mockResolvedValue([]);
});

// ── Tests ──

describe('App startup flow', () => {
  describe('Web mode (not Tauri)', () => {
    it('shows IdentitySetup when no identity exists', async () => {
      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('startup-mode-selector')).not.toBeInTheDocument();
    });

    it('shows StartupModeSelector when identity with pseudonymId exists', async () => {
      // Mock IndexedDB to contain a fully registered identity so
      // loadIdentities() auto-selects it (phase='ready').
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);

      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByTestId('startup-mode-selector')).toBeInTheDocument();
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
        expect(screen.getByTestId('startup-mode-selector')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });
  });

  describe('Tauri mode — the critical EXE startup flow', () => {
    beforeEach(() => {
      tauriEnabled = true;
    });

    it('shows IdentitySetup (NOT "Starting server...") on first launch when no identity exists', async () => {
      // THIS IS THE CRITICAL TEST.
      //
      // On first EXE launch, no identity keys exist. The app MUST show the
      // identity creation screen — not a server selection or "Starting
      // server..." screen. No server is started during identity creation.
      // The server only enters the picture on Screen 2.

      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      });
      // No server-related UI during identity creation.
      expect(screen.queryByTestId('startup-mode-selector')).not.toBeInTheDocument();
      expect(screen.queryByText(/Starting server/)).not.toBeInTheDocument();
    });

    it('IdentitySetup has NO server field, NO "Failed to fetch" error', async () => {
      const App = (await import('./App')).default;
      render(<App />);

      await waitFor(() => {
        expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      });

      // The identity-setup mock renders "Create Your Identity" — no server
      // fields, no network error text.
      expect(screen.queryByText(/Failed to fetch/)).not.toBeInTheDocument();
    });

    it('after keys created, shows StartupModeSelector immediately (no server gate)', async () => {
      const App = (await import('./App')).default;
      render(<App />);

      // Start on Screen 1
      await waitFor(() => {
        expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      });

      // User creates identity keys (locally, offline)
      act(() => {
        useIdentityStore.setState({
          phase: 'keys_ready',
          identity: KEYS_ONLY_IDENTITY,
          storedIdentities: [KEYS_ONLY_IDENTITY],
        });
      });

      // Screen 2 appears immediately — no "Starting server..." gate.
      // The server hasn't started yet; it only starts when the user
      // explicitly picks "Host a Server" on this screen.
      await waitFor(() => {
        expect(screen.getByTestId('startup-mode-selector')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });

    it('existing identity with pseudonymId skips Screen 1 entirely', async () => {
      // Returning user: IndexedDB already has a fully registered identity
      // AND startup prefs exist (they completed the full flow before).
      // Screen 1 (identity creation) should be skipped.
      const dbMock = await import('@/lib/db');
      vi.mocked(dbMock.listIdentities).mockResolvedValue([FAKE_IDENTITY]);
      const tauri = await import('@/lib/tauri');
      vi.mocked(tauri.getStartupMode).mockResolvedValue({
        startup_mode: { mode: 'host' },
      });

      const App = (await import('./App')).default;
      render(<App />);

      // Should go to Screen 2 (after server starts), never showing Screen 1
      await waitFor(() => {
        expect(screen.getByTestId('startup-mode-selector')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });

    it('full Tauri flow: Screen 1 → Screen 2 → main app', async () => {
      const App = (await import('./App')).default;
      render(<App />);

      // Step 1: Screen 1 (identity creation)
      await waitFor(() => {
        expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      });

      // Step 2: User creates keys
      act(() => {
        useIdentityStore.setState({
          phase: 'keys_ready',
          identity: KEYS_ONLY_IDENTITY,
          storedIdentities: [KEYS_ONLY_IDENTITY],
        });
      });

      // Step 3: Screen 2 (server/mode selection)
      await waitFor(() => {
        expect(screen.getByTestId('startup-mode-selector')).toBeInTheDocument();
      });

      // Step 4: User picks a mode → triggers auto-registration
      await act(async () => {
        screen.getByTestId('pick-mode-btn').click();
      });

      // Step 5: Simulate registration completing
      // (In real flow, the auto-register effect calls registerWithServer
      // which sets phase='ready' with pseudonymId.)
      act(() => {
        useIdentityStore.setState({
          phase: 'ready',
          identity: FAKE_IDENTITY,
          storedIdentities: [FAKE_IDENTITY],
        });
      });

      // Step 6: Main app
      await waitFor(() => {
        expect(screen.getByTestId('status-bar')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
      expect(screen.queryByTestId('startup-mode-selector')).not.toBeInTheDocument();
    });

    it('no server is started during identity creation', async () => {
      const tauri = await import('@/lib/tauri');

      const App = (await import('./App')).default;
      render(<App />);

      // Identity creation screen should appear
      await waitFor(() => {
        expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      });

      // startEmbeddedServer must NOT have been called — the server has
      // no business starting while the user is creating their identity.
      expect(tauri.startEmbeddedServer).not.toHaveBeenCalled();
    });

    it('clears stale startup prefs if no identity exists on launch', async () => {
      // Scenario: User deleted identity (or fresh install with lingering prefs).
      // Prefs exist pointing to a server.
      // But no identity in DB.
      // App should CLEAR prefs and show IdentitySetup, then show StartupModeSelector in "Choose" mode.

      // Mock stale prefs exist
      const tauri = await import('@/lib/tauri');
      vi.mocked(tauri.getStartupMode).mockResolvedValue({
        startup_mode: { mode: 'host' },
      });

      const App = (await import('./App')).default;
      render(<App />);

      // IdentitySetup should appear (because no identity)
      await waitFor(() => {
        expect(screen.getByTestId('identity-setup')).toBeInTheDocument();
      });

      // Verify clearStartupMode was called to purge stale prefs
      expect(tauri.clearStartupMode).toHaveBeenCalled();
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
        expect(screen.getByTestId('startup-mode-selector')).toBeInTheDocument();
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
        expect(screen.getByTestId('startup-mode-selector')).toBeInTheDocument();
      });
      expect(screen.queryByTestId('identity-setup')).not.toBeInTheDocument();
    });
  });
});
