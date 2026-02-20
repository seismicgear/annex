/**
 * Root application component.
 *
 * Orchestrates the identity flow, view navigation, and layout.
 * Shows IdentitySetup when no identity is active, otherwise shows
 * a tabbed layout with Chat, Federation, Events, and Admin views.
 */

import { useEffect, useState } from 'react';
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

type AppView = 'chat' | 'federation' | 'events' | 'admin';

export default function App() {
  const { phase, identity, loadIdentities, loadPermissions, permissions } = useIdentityStore();
  const { connectWs, disconnectWs } = useChannelsStore();
  const [activeView, setActiveView] = useState<AppView>('chat');

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
      case 'admin':
        return (
          <main className="view-content">
            <AdminPanel />
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
          {permissions?.can_moderate && (
            <button
              className={`tab-btn ${activeView === 'admin' ? 'active' : ''}`}
              onClick={() => setActiveView('admin')}
            >
              Admin
            </button>
          )}
        </nav>
      </header>
      {renderView()}
      <StatusBar />
    </div>
  );
}
