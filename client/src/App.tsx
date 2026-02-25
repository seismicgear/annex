/**
 * Root application component.
 *
 * Orchestrates the identity flow, view navigation, and layout.
 * Shows IdentitySetup when no identity is active, otherwise shows
 * a tabbed layout with Chat, Federation, and Events views.
 *
 * The ServerHub sidebar renders the user's local database of established
 * Merkle tree insertions — click-to-connect with immediate UI transitions
 * and async crypto in the background. Persona isolation dynamically
 * shifts the color palette based on the active server context.
 *
 * Admin features are accessible via a gear-icon dropdown menu.
 * Supports invite link routing: if the URL contains /invite/<channelId>,
 * the app will auto-join the channel after identity setup completes.
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
import { StartupModeSelector, clearWebStartupMode } from '@/components/StartupModeSelector';
import { parseInviteFromUrl, clearInviteFromUrl } from '@/lib/invite';
import { getPersonasForIdentity } from '@/lib/personas';
import { getApiBaseUrl, setApiBaseUrl, setPublicUrl } from '@/lib/api';
import { isTauri } from '@/lib/tauri';
import type { InvitePayload } from '@/types';
import './App.css';

type AppView = 'chat' | 'federation' | 'events' | 'admin-policy' | 'admin-channels' | 'admin-members' | 'admin-server';

export default function App() {
  const { phase, identity, loadIdentities, loadPermissions, permissions } = useIdentityStore();
  const { connectWs, disconnectWs, selectChannel, joinChannel, loadChannels } = useChannelsStore();
  const { servers, loadServers, saveCurrentServer, fetchServerImage } = useServersStore();
  const activeServer = useServersStore((s) => s.getActiveServer());
  const serverImageUrl = useServersStore((s) => s.serverImageUrl);
  const inTauri = isTauri();
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

  // Tauri: tracks the auto-started embedded server URL.
  const [embeddedServerUrl, setEmbeddedServerUrl] = useState<string | null>(null);
  const [embeddedServerError, setEmbeddedServerError] = useState<string | null>(null);
  const [serverRetry, setServerRetry] = useState(0);

  // Load identities and saved servers on mount
  useEffect(() => {
    loadIdentities();
    loadServers();
  }, [loadIdentities, loadServers]);

  // Tauri: auto-start the embedded server immediately so the identity
  // creation screen (which needs a running server for registration and
  // proof verification) can be shown first, before the host/connect choice.
  useEffect(() => {
    if (!inTauri) return;
    let cancelled = false;
    setEmbeddedServerError(null);
    (async () => {
      try {
        const { startEmbeddedServer } = await import('@/lib/tauri');
        const url = await startEmbeddedServer();
        if (!cancelled) {
          setApiBaseUrl(url);
          setEmbeddedServerUrl(url);
        }
      } catch (err) {
        if (!cancelled) {
          setEmbeddedServerError(
            err instanceof Error ? err.message : String(err),
          );
        }
      }
    })();
    return () => { cancelled = true; };
  }, [inTauri, serverRetry]);

  // When the user logs out, return to the mode selector.
  // We track the previous phase so we only reset when phase *transitions*
  // to 'uninitialized' (a real logout), not when it was already 'uninitialized'
  // (e.g. user just picked a server and hasn't created an identity yet).
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
      // Save this server to the local hub (idempotent — skips if already saved)
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
    // Validate hex color format; fall back to default if malformed
    const accentColor = /^#[0-9a-fA-F]{6}$/.test(raw) ? raw : '#e63946';
    document.documentElement.style.setProperty('--persona-accent', accentColor);

    // Derive tint colors from the accent
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

  // ── Tauri: loading while embedded server auto-starts ──
  if (inTauri && !embeddedServerUrl && !embeddedServerError) {
    return (
      <div className="app">
        <main className="app-main setup">
          <div className="startup-mode-selector">
            <h2>Annex</h2>
            <div className="startup-loading">Starting server...</div>
          </div>
        </main>
      </div>
    );
  }

  // ── Tauri: embedded server failed to start ──
  if (inTauri && embeddedServerError) {
    return (
      <div className="app">
        <main className="app-main setup">
          <div className="startup-mode-selector">
            <h2>Annex</h2>
            <div className="error-message">{embeddedServerError}</div>
            <button className="primary-btn" onClick={() => setServerRetry((n) => n + 1)}>
              Retry
            </button>
          </div>
        </main>
      </div>
    );
  }

  // ── Identity setup (both Tauri and Web) ──
  if (phase !== 'ready' || !identity?.pseudonymId) {
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
          <IdentitySetup inviteServerSlug={pendingInvite?.serverSlug} />
        </main>
      </div>
    );
  }

  // ── Mode selection (host / connect) after identity is ready ──
  if (!serverReady) {
    return (
      <div className="app">
        <main className="app-main setup">
          <StartupModeSelector
            embeddedServerRunning={inTauri}
            onReady={(url) => { setTunnelUrl(url ?? null); setServerReady(true); }}
          />
        </main>
      </div>
    );
  }

  const navigateAdmin = (view: AppView) => {
    setActiveView(view);
    setAdminMenuOpen(false);
  };

  // Render active view content
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

        {/* Persona context indicator — shows which mask is active */}
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
        {/* Server Hub — the leftmost icon sidebar showing saved Merkle insertions */}
        {servers.length > 0 && <ServerHub />}

        {/* Main content area — shifts color palette per active persona */}
        <div className="app-main-content" key={activeServer?.id ?? 'default'}>
          {/* VoicePanel is rendered outside the view switch so the call
              stays connected when the user navigates to Federation/Events. */}
          <VoicePanel />
          {renderView()}
        </div>
      </div>

      <StatusBar tunnelUrl={tunnelUrl} />
    </div>
  );
}
