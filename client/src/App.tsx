/**
 * Root application component.
 *
 * Orchestrates the startup flow:
 *   Screen 1  – Identity creation (offline, zero network requests, no server)
 *   Screen 2  – Server / startup-mode selection (server starts here, not before)
 *
 * Identity creation is purely local — keys are generated and stored on the
 * device. No server is started, contacted, or registered with during
 * identity creation. The server only enters the picture when the user
 * explicitly chooses a server mode on Screen 2.
 */

import { useEffect, useState, useRef } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
import { useServersStore } from '@/stores/servers';
import { IdentitySetup } from '@/components/IdentitySetup';
import { ChannelList } from '@/components/ChannelList';
import { MessageView } from '@/components/MessageView';
import { MessageInput } from '@/components/MessageInput';
import { VoicePanel } from '@/components/VoicePanel';
import { MemberList } from '@/components/MemberList';
import { StatusBar } from '@/components/StatusBar';
import { FederationPanel } from '@/components/FederationPanel';
import { EventLog } from '@/components/EventLog';
import { AdminPanel } from '@/components/AdminPanel';
import { ServerHub } from '@/components/ServerHub';
import { StartupModeSelector } from '@/components/StartupModeSelector';
import { clearWebStartupMode } from '@/lib/startup-prefs';
import { parseInviteFromUrl, clearInviteFromUrl } from '@/lib/invite';
import { getPersonasForIdentity } from '@/lib/personas';
import { getApiBaseUrl, getServerSummary, setPublicUrl } from '@/lib/api';
import { isTauri, getStartupMode as tauriGetStartupMode, clearStartupMode as tauriClearStartupMode } from '@/lib/tauri';
import type { InvitePayload } from '@/types';
import './App.css';

type AppView = 'chat' | 'federation' | 'events' | 'admin-policy' | 'admin-channels' | 'admin-members' | 'admin-server';

/** Labels shown while registering keys with the chosen server. */
const REGISTRATION_LABELS: Record<string, string> = {
  keys_ready: 'Preparing to register...',
  registering: 'Registering with server...',
  proving: 'Generating zero-knowledge proof...',
  verifying: 'Verifying membership...',
};

export default function App() {
  const { phase, identity, error, loadIdentities, loadPermissions, permissions, registerWithServer } = useIdentityStore();
  const { connectWs, disconnectWs, selectChannel, joinChannel, loadChannels } = useChannelsStore();
  const { servers, loadServers, saveCurrentServer, fetchServerImage } = useServersStore();
  const activeServer = useServersStore((s) => s.getActiveServer());
  const serverImageUrl = useServersStore((s) => s.serverImageUrl);
  const inTauri = isTauri();

  // Whether we have finished checking IndexedDB for existing identities.
  const [identityChecked, setIdentityChecked] = useState(false);

  const [serverReady, setServerReady] = useState(false);
  const [tunnelUrl, setTunnelUrl] = useState<string | null>(null);
  const [activeView, setActiveView] = useState<AppView>('chat');
  const [adminMenuOpen, setAdminMenuOpen] = useState(false);
  const adminMenuRef = useRef<HTMLDivElement>(null);
  const [pendingInvite, setPendingInvite] = useState<InvitePayload | null>(
    () => parseInviteFromUrl(),
  );
  const inviteProcessed = useRef(false);
  const serverSaved = useRef(false);
  const prevPhaseRef = useRef(phase);

  // ── Load identities + servers on mount (all modes) ──
  // In Tauri mode, after loading identities we also check whether startup
  // preferences (startup_prefs.json) exist.  If they don't, the user has
  // never completed the full setup flow — reset identity selection so
  // IdentitySetup renders first, even if IndexedDB has a valid identity
  // from a previous install.  Returning users with saved prefs skip this.
  useEffect(() => {
    loadIdentities()
      .then(async () => {
        if (inTauri) {
          // If no identity exists, clear any stale startup preferences so the user
          // is forced to choose a server mode (Host vs Connect) after creating keys.
          // This satisfies the requirement: Identity Creation -> Server Choice.
          const { identity } = useIdentityStore.getState();
          if (!identity) {
            await tauriClearStartupMode().catch(() => {});
            return null;
          }
          return tauriGetStartupMode().catch(() => null);
        }
        return undefined;
      })
      .then((startupPrefs) => {
        if (inTauri && startupPrefs === null) {
          const { phase: currentPhase } = useIdentityStore.getState();
          if (currentPhase === 'ready') {
            useIdentityStore.getState().logout();
          }
        }
        setIdentityChecked(true);
      });
    loadServers();
  }, [loadIdentities, loadServers, inTauri]);

  // ── Register identity with server after user selects a server ──
  // Only fires when phase is exactly 'keys_ready' (keys exist, not yet
  // registered) and the user has explicitly picked a server on Screen 2.
  //
  // Retries getServerSummary() with exponential backoff because the
  // server may still be initialising when serverReady flips to true.
  useEffect(() => {
    if (!serverReady || phase !== 'keys_ready' || !identity?.sk) return;
    let cancelled = false;

    const MAX_RETRIES = 5;
    const BASE_DELAY_MS = 500;

    (async () => {
      let lastError: unknown;
      for (let attempt = 0; attempt <= MAX_RETRIES; attempt++) {
        if (cancelled) return;
        try {
          const summary = await getServerSummary();
          if (!cancelled) {
            await registerWithServer(summary.slug);
          }
          return;
        } catch (err) {
          lastError = err;
          if (attempt < MAX_RETRIES) {
            await new Promise((r) => setTimeout(r, BASE_DELAY_MS * 2 ** attempt));
          }
        }
      }
      if (!cancelled) {
        useIdentityStore.setState({
          phase: 'error',
          error: lastError instanceof Error ? lastError.message : 'Failed to reach server',
        });
      }
    })();
    return () => { cancelled = true; };
  }, [serverReady, phase, identity?.sk, registerWithServer]);

  // When the user logs out, return to the mode selector.
  // We track the previous phase so we only reset when phase *transitions*
  // to 'uninitialized' (a real logout), not when it was already 'uninitialized'.
  useEffect(() => {
    const prevPhase = prevPhaseRef.current;
    prevPhaseRef.current = phase;

    if (phase === 'uninitialized' && prevPhase !== 'uninitialized' && serverReady) {
      clearWebStartupMode();
      setServerReady(false);
      serverSaved.current = false;
    }
  }, [phase, serverReady]);

  // Connect WebSocket and load permissions when identity is ready
  useEffect(() => {
    if (phase === 'ready' && identity?.pseudonymId) {
      const baseUrl = getApiBaseUrl();
      connectWs(identity.pseudonymId, baseUrl || undefined);
      loadPermissions();
      fetchServerImage();
      return () => disconnectWs();
    }
  }, [phase, identity?.pseudonymId, connectWs, disconnectWs, loadPermissions, fetchServerImage]);

  // Push tunnel URL to the server so invite links, federation, and relay
  // paths use the globally-reachable address instead of localhost.
  useEffect(() => {
    if (tunnelUrl && phase === 'ready' && identity?.pseudonymId && permissions?.capabilities.can_moderate) {
      setPublicUrl(identity.pseudonymId, tunnelUrl).catch(() => {
        // Non-fatal: admin-only endpoint — will fail for non-admins silently
      });
    }
  }, [tunnelUrl, phase, identity?.pseudonymId, permissions?.capabilities.can_moderate]);

  // Auto-save current server to the node hub on first identity ready
  useEffect(() => {
    if (phase === 'ready' && identity?.pseudonymId && identity.id && !serverSaved.current) {
      serverSaved.current = true;
      saveCurrentServer(identity.id, identity.serverSlug, identity.serverSlug)
        .then(() =>
          getPersonasForIdentity(identity.id).then((personas) => {
            if (personas.length > 0) {
              const server = useServersStore.getState().getActiveServer();
              if (server && !server.personaId) {
                useServersStore.getState().setServerPersona(
                  server.id,
                  personas[0].id,
                  personas[0].accentColor,
                );
              }
            }
          }),
        )
        .catch(() => {
          // Non-fatal: server hub entry may not be saved on first load
        });
    }
  }, [phase, identity?.pseudonymId, identity?.id, identity?.serverSlug, saveCurrentServer]);

  // Apply persona isolation — dynamic CSS custom properties per server context
  useEffect(() => {
    const raw = activeServer?.accentColor ?? '#e63946';
    const accentColor = /^#[0-9a-fA-F]{6}$/.test(raw) ? raw : '#e63946';
    document.documentElement.style.setProperty('--persona-accent', accentColor);

    const r = parseInt(accentColor.slice(1, 3), 16);
    const g = parseInt(accentColor.slice(3, 5), 16);
    const b = parseInt(accentColor.slice(5, 7), 16);
    document.documentElement.style.setProperty(
      '--persona-bg-tint',
      `rgba(${r}, ${g}, ${b}, 0.06)`,
    );
    document.documentElement.style.setProperty(
      '--persona-border-tint',
      `rgba(${r}, ${g}, ${b}, 0.3)`,
    );
  }, [activeServer?.accentColor]);

  // Process invite after identity is ready
  useEffect(() => {
    if (
      phase === 'ready' &&
      identity?.pseudonymId &&
      pendingInvite &&
      !inviteProcessed.current
    ) {
      inviteProcessed.current = true;
      const processInvite = async () => {
        try {
          await joinChannel(identity.pseudonymId!, pendingInvite.channelId).catch(() => {
            // Expected: channel might already be joined
          });
          await loadChannels(identity.pseudonymId!);
          selectChannel(identity.pseudonymId!, pendingInvite.channelId);
        } finally {
          clearInviteFromUrl();
          setActiveView('chat');
          setPendingInvite(null);
        }
      };
      processInvite().catch(() => {
        // Non-fatal: invite processing failed, user lands on chat view
      });
    }
  }, [phase, identity?.pseudonymId, pendingInvite, joinChannel, loadChannels, selectChannel]);

  // Close admin menu on outside click
  useEffect(() => {
    if (!adminMenuOpen) return;
    const handler = (e: MouseEvent) => {
      if (adminMenuRef.current && !adminMenuRef.current.contains(e.target as Node)) {
        setAdminMenuOpen(false);
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [adminMenuOpen]);

  // ────────────────────────────────────────────────────────────────────
  // RENDER GATES — evaluated top-to-bottom, first match wins.
  // ────────────────────────────────────────────────────────────────────

  // Gate 0: Still checking IndexedDB for existing identities.
  if (!identityChecked) {
    return (
      <div className="app">
        <main className="app-main setup">
          <div className="startup-mode-selector">
            <h2>Annex</h2>
            <div className="startup-loading">Loading...</div>
          </div>
        </main>
      </div>
    );
  }

  // Gate 1 — HARD GATE: No identity keys → Screen 1 (identity creation).
  // This screen makes ZERO network requests.
  if (!identity?.sk) {
    return (
      <div className="app">
        <header className="app-header">
          <h1>Annex</h1>
          {pendingInvite && (
            <span className="invite-banner">
              Joining {pendingInvite.label ?? pendingInvite.channelId}...
            </span>
          )}
        </header>
        <main className="app-main setup">
          <IdentitySetup />
        </main>
      </div>
    );
  }

  // Gate 2: Server not yet selected → Screen 2 (startup mode selector).
  // The server is NOT started yet — StartupModeSelector handles starting
  // the embedded server if the user picks "Host a Server".
  if (!serverReady) {
    return (
      <div className="app">
        <main className="app-main setup">
          <StartupModeSelector
            onReady={(url) => { setTunnelUrl(url ?? null); setServerReady(true); }}
          />
        </main>
      </div>
    );
  }

  // Gate 3: Identity keys not yet registered with the chosen server.
  // The auto-register effect handles the registration automatically;
  // this gate just shows progress while it runs.
  if (phase !== 'ready' || !identity?.pseudonymId) {
    return (
      <div className="app">
        <main className="app-main setup">
          <div className="identity-setup">
            <h2>Annex</h2>
            <div className={`phase-status phase-${phase}`}>
              {REGISTRATION_LABELS[phase] ?? 'Preparing...'}
            </div>
            {phase === 'error' && error && (
              <>
                <div className="error-message">{error}</div>
                <button
                  className="primary-btn"
                  onClick={() => {
                    useIdentityStore.setState({ phase: 'keys_ready', error: null });
                  }}
                >
                  Retry
                </button>
              </>
            )}
          </div>
        </main>
      </div>
    );
  }

  // ────────────────────────────────────────────────────────────────────
  // MAIN APP — identity ready, server connected.
  // ────────────────────────────────────────────────────────────────────

  const navigateAdmin = (view: AppView) => {
    setActiveView(view);
    setAdminMenuOpen(false);
  };

  const renderView = () => {
    switch (activeView) {
      case 'federation':
        return (
          <main className="view-content">
            <FederationPanel />
          </main>
        );
      case 'events':
        return (
          <main className="view-content">
            <EventLog />
          </main>
        );
      case 'admin-policy':
      case 'admin-channels':
      case 'admin-members':
      case 'admin-server': {
        const sectionMap: Record<string, 'policy' | 'channels' | 'members' | 'server'> = {
          'admin-policy': 'policy',
          'admin-channels': 'channels',
          'admin-members': 'members',
          'admin-server': 'server',
        };
        return (
          <main className="view-content">
            <AdminPanel section={sectionMap[activeView]} />
          </main>
        );
      }
      default:
        return (
          <div className="app-layout">
            <aside className="sidebar-left">
              <ChannelList />
            </aside>
            <main className="chat-area">
              <MessageView />
              <MessageInput />
            </main>
            <aside className="sidebar-right">
              <MemberList />
            </aside>
          </div>
        );
    }
  };

  return (
    <div className="app">
      <header className="app-header">
        {serverImageUrl && (
          <img src={serverImageUrl} alt="" className="header-server-image" />
        )}
        <h1>Annex</h1>
        <nav className="header-tabs">
          <button
            className={`tab-btn ${activeView === 'chat' ? 'active' : ''}`}
            onClick={() => setActiveView('chat')}
          >
            Chat
          </button>
          <button
            className={`tab-btn ${activeView === 'federation' ? 'active' : ''}`}
            onClick={() => setActiveView('federation')}
          >
            Federation
          </button>
          <button
            className={`tab-btn ${activeView === 'events' ? 'active' : ''}`}
            onClick={() => setActiveView('events')}
          >
            Events
          </button>
        </nav>

        {activeServer && (
          <div className="persona-context-indicator">
            <span className="persona-context-dot" />
            <span className="persona-context-name">
              {activeServer.label}
            </span>
            <span className="persona-context-server">
              {activeServer.slug}
            </span>
          </div>
        )}

        {permissions?.capabilities.can_moderate && (
          <div className="admin-menu" ref={adminMenuRef}>
            <button
              className={`admin-menu-btn ${activeView.startsWith('admin') ? 'active' : ''}`}
              onClick={() => setAdminMenuOpen((o) => !o)}
              title="Admin"
            >
              <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
                <path d="M8 4.754a3.246 3.246 0 1 0 0 6.492 3.246 3.246 0 0 0 0-6.492zM5.754 8a2.246 2.246 0 1 1 4.492 0 2.246 2.246 0 0 1-4.492 0z"/>
                <path d="M9.796 1.343c-.527-1.79-3.065-1.79-3.592 0l-.094.319a.873.873 0 0 1-1.255.52l-.292-.16c-1.64-.892-3.433.902-2.54 2.541l.159.292a.873.873 0 0 1-.52 1.255l-.319.094c-1.79.527-1.79 3.065 0 3.592l.319.094a.873.873 0 0 1 .52 1.255l-.16.292c-.892 1.64.901 3.434 2.541 2.54l.292-.159a.873.873 0 0 1 1.255.52l.094.319c.527 1.79 3.065 1.79 3.592 0l.094-.319a.873.873 0 0 1 1.255-.52l.292.16c1.64.893 3.434-.902 2.54-2.541l-.159-.292a.873.873 0 0 1 .52-1.255l.319-.094c1.79-.527 1.79-3.065 0-3.592l-.319-.094a.873.873 0 0 1-.52-1.255l.16-.292c.893-1.64-.902-3.433-2.541-2.54l-.292.159a.873.873 0 0 1-1.255-.52l-.094-.319zm-2.633.283c.246-.835 1.428-.835 1.674 0l.094.319a1.873 1.873 0 0 0 2.693 1.115l.291-.16c.764-.415 1.6.42 1.184 1.185l-.159.292a1.873 1.873 0 0 0 1.116 2.692l.318.094c.835.246.835 1.428 0 1.674l-.319.094a1.873 1.873 0 0 0-1.115 2.693l.16.291c.415.764-.421 1.6-1.185 1.184l-.291-.159a1.873 1.873 0 0 0-2.693 1.116l-.094.318c-.246.835-1.428.835-1.674 0l-.094-.319a1.873 1.873 0 0 0-2.692-1.115l-.292.16c-.764.415-1.6-.421-1.184-1.185l.159-.291A1.873 1.873 0 0 0 1.945 8.93l-.319-.094c-.835-.246-.835-1.428 0-1.674l.319-.094A1.873 1.873 0 0 0 3.06 4.377l-.16-.292c-.415-.764.42-1.6 1.185-1.184l.292.159a1.873 1.873 0 0 0 2.692-1.116l.094-.318z"/>
              </svg>
            </button>
            {adminMenuOpen && (
              <div className="admin-dropdown">
                <button
                  className={`admin-dropdown-item ${activeView === 'admin-server' ? 'active' : ''}`}
                  onClick={() => navigateAdmin('admin-server')}
                >
                  Server Settings
                </button>
                <button
                  className={`admin-dropdown-item ${activeView === 'admin-policy' ? 'active' : ''}`}
                  onClick={() => navigateAdmin('admin-policy')}
                >
                  Server Policy
                </button>
                <button
                  className={`admin-dropdown-item ${activeView === 'admin-members' ? 'active' : ''}`}
                  onClick={() => navigateAdmin('admin-members')}
                >
                  Member Management
                </button>
                <button
                  className={`admin-dropdown-item ${activeView === 'admin-channels' ? 'active' : ''}`}
                  onClick={() => navigateAdmin('admin-channels')}
                >
                  Channel Management
                </button>
              </div>
            )}
          </div>
        )}
      </header>

      <div className="app-with-hub">
        {servers.length > 0 && <ServerHub />}
        <div className="app-main-content" key={activeServer?.id ?? 'default'}>
          <VoicePanel />
          {renderView()}
        </div>
      </div>

      <StatusBar tunnelUrl={tunnelUrl} />
    </div>
  );
}
