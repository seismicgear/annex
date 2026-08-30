/**
 * Main app layout shown after the user is registered with a server.
 *
 * Renders the header (logo, tab bar, persona context, admin menu), the
 * protocol-invite confirmation banner (if one arrived during an active
 * session), degraded-startup notices, the reconnection banner, the
 * server hub, the voice panel, and the active view (chat / federation /
 * events / admin sections).
 */

import { useEffect, useRef, useState, type Dispatch, type ReactNode, type SetStateAction } from 'react';
import { AdminPanel } from '@/components/AdminPanel';
import { ErrorBoundary } from '@/components/ErrorBoundary';
import { ChannelList } from '@/components/ChannelList';
import { ChannelEncryptionBar } from '@/components/ChannelEncryptionBar';
import { EventLog } from '@/components/EventLog';
import { FederationPanel } from '@/components/FederationPanel';
import { MemberList } from '@/components/MemberList';
import { MessageInput } from '@/components/MessageInput';
import { MessageSearch } from '@/components/MessageSearch';
import { MessageView } from '@/components/MessageView';
import { ServerHub } from '@/components/ServerHub';
import { StatusBar } from '@/components/StatusBar';
import type { DegradedStartupInfo } from '@/components/StartupModeSelector';
import { VoicePanel } from '@/components/VoicePanel';
import type { PermissionsStatus } from '@/stores/identity';
import type { SavedServer } from '@/types';
import type { InvitePayload } from '@/types';

export type AppView =
  | 'chat'
  | 'federation'
  | 'events'
  | 'admin-policy'
  | 'admin-channels'
  | 'admin-members'
  | 'admin-server'
  | 'admin-federation';

export interface MainLayoutProps {
  activeView: AppView;
  setActiveView: Dispatch<SetStateAction<AppView>>;
  activeServer: SavedServer | null;
  servers: SavedServer[];
  serverImageUrl: string | null;
  canModerate: boolean;
  permissionsStatus: PermissionsStatus;
  loadPermissions: () => Promise<void>;
  pendingProtocolInviteConfirmation: InvitePayload | null;
  handleAcceptProtocolInvite: () => Promise<void>;
  handleIgnoreProtocolInvite: () => void;
  degradedStartup: DegradedStartupInfo | null;
  setDegradedStartup: Dispatch<SetStateAction<DegradedStartupInfo | null>>;
  reconnectionBanner: ReactNode;
}

export function MainLayout({
  activeView,
  setActiveView,
  activeServer,
  servers,
  serverImageUrl,
  canModerate,
  permissionsStatus,
  loadPermissions,
  pendingProtocolInviteConfirmation,
  handleAcceptProtocolInvite,
  handleIgnoreProtocolInvite,
  degradedStartup,
  setDegradedStartup,
  reconnectionBanner,
}: MainLayoutProps) {
  const [adminMenuOpen, setAdminMenuOpen] = useState(false);
  const adminMenuRef = useRef<HTMLDivElement>(null);

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

  const navigateAdmin = (view: AppView) => {
    setActiveView(view);
    setAdminMenuOpen(false);
  };

  const renderView = () => {
    switch (activeView) {
      case 'federation':
        return (
          <main className="view-content">
            <ErrorBoundary label="Federation">
              <FederationPanel />
            </ErrorBoundary>
          </main>
        );
      case 'events':
        return (
          <main className="view-content">
            <ErrorBoundary label="Events">
              <EventLog />
            </ErrorBoundary>
          </main>
        );
      case 'admin-policy':
      case 'admin-channels':
      case 'admin-members':
      case 'admin-server':
      case 'admin-federation': {
        const sectionMap: Record<
          string,
          'policy' | 'channels' | 'members' | 'server' | 'federation'
        > = {
          'admin-policy': 'policy',
          'admin-channels': 'channels',
          'admin-members': 'members',
          'admin-server': 'server',
          'admin-federation': 'federation',
        };
        return (
          <main className="view-content">
            <ErrorBoundary label="Admin">
              <AdminPanel section={sectionMap[activeView]} />
            </ErrorBoundary>
          </main>
        );
      }
      default:
        return (
          <div className="app-layout">
            <aside className="sidebar-left" aria-label="Channels">
              <ErrorBoundary label="the channel list">
                <ChannelList />
              </ErrorBoundary>
            </aside>
            <main className="chat-area" aria-label="Conversation">
              {/* Separate boundaries: a crash in the message list must not
                  take the composer with it, and vice versa. */}
              <ErrorBoundary label="the conversation">
                <MessageSearch />
                <ChannelEncryptionBar />
                <MessageView />
              </ErrorBoundary>
              <ErrorBoundary label="the composer">
                <MessageInput />
              </ErrorBoundary>
            </main>
            <aside className="sidebar-right" aria-label="Members and agents">
              <ErrorBoundary label="the member list">
                <MemberList />
              </ErrorBoundary>
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
        <nav className="header-tabs" aria-label="Main views">
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

        {(canModerate || permissionsStatus === 'loading' || permissionsStatus === 'error') && (
          <div className="admin-menu" ref={adminMenuRef}>
            <button
              className={`admin-menu-btn ${activeView.startsWith('admin') ? 'active' : ''} ${permissionsStatus === 'loading' ? 'loading' : ''}`}
              onClick={() => {
                if (permissionsStatus === 'error') {
                  void loadPermissions();
                } else if (canModerate) {
                  setAdminMenuOpen((o) => !o);
                }
              }}
              title={
                permissionsStatus === 'loading'
                  ? 'Loading permissions…'
                  : permissionsStatus === 'error'
                    ? 'Failed to load permissions — click to retry'
                    : 'Admin'
              }
              disabled={permissionsStatus === 'loading'}
            >
              <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
                <path d="M8 4.754a3.246 3.246 0 1 0 0 6.492 3.246 3.246 0 0 0 0-6.492zM5.754 8a2.246 2.246 0 1 1 4.492 0 2.246 2.246 0 0 1-4.492 0z"/>
                <path d="M9.796 1.343c-.527-1.79-3.065-1.79-3.592 0l-.094.319a.873.873 0 0 1-1.255.52l-.292-.16c-1.64-.892-3.433.902-2.54 2.541l.159.292a.873.873 0 0 1-.52 1.255l-.319.094c-1.79.527-1.79 3.065 0 3.592l.319.094a.873.873 0 0 1 .52 1.255l-.16.292c-.892 1.64.901 3.434 2.541 2.54l.292-.159a.873.873 0 0 1 1.255.52l.094.319c.527 1.79 3.065 1.79 3.592 0l.094-.319a.873.873 0 0 1 1.255-.52l.292.16c1.64.893 3.434-.902 2.54-2.541l-.159-.292a.873.873 0 0 1 .52-1.255l.319-.094c1.79-.527 1.79-3.065 0-3.592l-.319-.094a.873.873 0 0 1-.52-1.255l.16-.292c.893-1.64-.902-3.433-2.541-2.54l-.292.159a.873.873 0 0 1-1.255-.52l-.094-.319zm-2.633.283c.246-.835 1.428-.835 1.674 0l.094.319a1.873 1.873 0 0 0 2.693 1.115l.291-.16c.764-.415 1.6.42 1.184 1.185l-.159.292a1.873 1.873 0 0 0 1.116 2.692l.318.094c.835.246.835 1.428 0 1.674l-.319.094a1.873 1.873 0 0 0-1.115 2.693l.16.291c.415.764-.421 1.6-1.185 1.184l-.291-.159a1.873 1.873 0 0 0-2.693 1.116l-.094.318c-.246.835-1.428.835-1.674 0l-.094-.319a1.873 1.873 0 0 0-2.692-1.115l-.292.16c-.764.415-1.6-.421-1.184-1.185l.159-.291A1.873 1.873 0 0 0 1.945 8.93l-.319-.094c-.835-.246-.835-1.428 0-1.674l.319-.094A1.873 1.873 0 0 0 3.06 4.377l-.16-.292c-.415-.764.42-1.6 1.185-1.184l.292.159a1.873 1.873 0 0 0 2.692-1.116l.094-.318z"/>
              </svg>
            </button>
            {adminMenuOpen && canModerate && (
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
                <button
                  className={`admin-dropdown-item ${activeView === 'admin-federation' ? 'active' : ''}`}
                  onClick={() => navigateAdmin('admin-federation')}
                >
                  Federation Delivery
                </button>
              </div>
            )}
          </div>
        )}
      </header>

      {/*
        A banner, and now labelled as one. It declared `role="dialog"` while
        sitting in normal flow with no overlay, no focus moved into it, no
        focus trap and no Escape — telling assistive tech it was a modal when
        nothing about it behaved like one, and inconsistent with the
        degraded-startup banner directly below, which has always been a
        `status`. `region` plus `aria-live` announces it when it appears and
        makes it navigable as a landmark, which is what it actually is.

        Making it a genuine modal is a defensible product change — accepting
        an `annex://` invite is consequential — but that is a decision about
        interrupting the user, not a role attribute.
      */}
      {pendingProtocolInviteConfirmation && (
        <div className="invite-confirmation-banner" role="region" aria-live="polite" aria-label="Invite confirmation">
          <span>
            Invite received for {pendingProtocolInviteConfirmation.server}
          </span>
          <button className="primary-btn" onClick={handleAcceptProtocolInvite}>
            Join invite server
          </button>
          <button className="secondary-btn" onClick={handleIgnoreProtocolInvite}>
            Ignore
          </button>
        </div>
      )}

      {degradedStartup && (
        <div className="degraded-startup-banner" role="status">
          {degradedStartup.voiceFailed && (
            <span>Server started, but voice is unavailable{degradedStartup.voiceError ? `: ${degradedStartup.voiceError}` : ''}.</span>
          )}
          {degradedStartup.publicEndpointFailed && (
            <span>Server started, but public invites are unavailable{degradedStartup.publicEndpointError ? `: ${degradedStartup.publicEndpointError}` : ''}.</span>
          )}
          {degradedStartup.webrtcRouteUnavailable && (
            <span>Public endpoint acquired, but remote voice/video is unavailable — the router does not proxy WebRTC traffic.</span>
          )}
          <button onClick={() => setDegradedStartup(null)} className="dismiss-banner-btn" aria-label="Dismiss">&times;</button>
        </div>
      )}
      {reconnectionBanner}
      <div className="app-with-hub">
        {servers.length > 0 && <ServerHub />}
        <div className="app-main-content" key={activeServer?.id ?? 'default'}>
          <VoicePanel />
          {renderView()}
        </div>
      </div>

      <StatusBar />
    </div>
  );
}
