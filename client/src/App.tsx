/**
 * Root application component.
 *
 * Orchestrates the identity flow, channel view, and sidebar panels.
 * Shows IdentitySetup when no identity is active, otherwise shows
 * the full chat layout.
 */

import { useEffect } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
import { IdentitySetup } from '@/components/IdentitySetup';
import { ChannelList } from '@/components/ChannelList';
import { MessageView } from '@/components/MessageView';
import { MessageInput } from '@/components/MessageInput';
import { VoicePanel } from '@/components/VoicePanel';
import { MemberList } from '@/components/MemberList';
import { StatusBar } from '@/components/StatusBar';
import './App.css';

export default function App() {
  const { phase, identity, loadIdentities } = useIdentityStore();
  const { connectWs, disconnectWs } = useChannelsStore();

  // Load identities on mount
  useEffect(() => {
    loadIdentities();
  }, [loadIdentities]);

  // Connect WebSocket when identity is ready
  useEffect(() => {
    if (phase === 'ready' && identity?.pseudonymId) {
      connectWs(identity.pseudonymId);
      return () => disconnectWs();
    }
  }, [phase, identity?.pseudonymId, connectWs, disconnectWs]);

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

  // Main chat layout
  return (
    <div className="app">
      <header className="app-header">
        <h1>Annex</h1>
      </header>
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
      <StatusBar />
    </div>
  );
}
