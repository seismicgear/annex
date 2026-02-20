/**
 * Root application component.
 *
 * Orchestrates the identity flow, view navigation, and layout.
 * Shows IdentitySetup when no identity is active, otherwise shows
 * a tabbed layout with Chat, Federation, and Events views.
 * Admin features are accessible via a gear-icon dropdown menu.
 */

import { useEffect, useState, useRef } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
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
import './App.css';

type AppView = 'chat' | 'federation' | 'events' | 'admin-policy' | 'admin-channels';

export default function App() {
  const { phase, identity, loadIdentities, loadPermissions, permissions } = useIdentityStore();
  const { connectWs, disconnectWs } = useChannelsStore();
  const [activeView, setActiveView] = useState<AppView>('chat');
  const [adminMenuOpen, setAdminMenuOpen] = useState(false);
  const adminMenuRef = useRef<HTMLDivElement>(null);

  // Load identities on mount
  useEffect(() => {
    loadIdentities();
  }, [loadIdentities]);

  // Connect WebSocket and load permissions when identity is ready
  useEffect(() => {
    if (phase === 'ready' && identity?.pseudonymId) {
      connectWs(identity.pseudonymId);
      loadPermissions();
      return () => disconnectWs();
    }
  }, [phase, identity?.pseudonymId, connectWs, disconnectWs, loadPermissions]);

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

  // Show identity setup if not ready
  if (phase !== 'ready' || !identity?.pseudonymId) {
    return (
      <div className="app">
        <header className="app-header">
          <h1>Annex</h1>
        </header>
        <main className="app-main setup">
          <IdentitySetup />
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
        return (
          <main className="view-content">
            <AdminPanel section={activeView === 'admin-policy' ? 'policy' : 'channels'} />
          </main>
        );
      default:
        return (
          <div className="app-layout">
            <aside className="sidebar-left">
              <ChannelList />
            </aside>
            <main className="chat-area">
              <VoicePanel />
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
                  className={`admin-dropdown-item ${activeView === 'admin-policy' ? 'active' : ''}`}
                  onClick={() => navigateAdmin('admin-policy')}
                >
                  Server Policy
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
      {renderView()}
      <StatusBar />
    </div>
  );
}
