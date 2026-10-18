import { useEffect } from 'react';
import { cleanup, render } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { MainLayout, type MainLayoutProps } from './MainLayout';
import type { SavedServer } from '@/types';

const lifetime = vi.hoisted(() => ({ start: vi.fn(), stop: vi.fn() }));
vi.mock('@/components/VoicePanel', () => ({ VoicePanel: () => {
  useEffect(() => { lifetime.start(); return () => lifetime.stop(); }, []);
  return <div>Active call</div>;
} }));
vi.mock('@/components/AdminPanel', () => ({ AdminPanel: () => null }));
vi.mock('@/components/ChannelList', () => ({ ChannelList: () => null }));
vi.mock('@/components/ChannelEncryptionBar', () => ({ ChannelEncryptionBar: () => null }));
vi.mock('@/components/EventLog', () => ({ EventLog: () => null }));
vi.mock('@/components/FederationPanel', () => ({ FederationPanel: () => null }));
vi.mock('@/components/MemberList', () => ({ MemberList: () => null }));
vi.mock('@/components/MessageInput', () => ({ MessageInput: () => null }));
vi.mock('@/components/MessageSearch', () => ({ MessageSearch: () => null }));
vi.mock('@/components/MessageView', () => ({ MessageView: () => null }));
vi.mock('@/components/ServerHub', () => ({ ServerHub: () => null }));
vi.mock('@/components/StatusBar', () => ({ StatusBar: () => null }));

afterEach(() => { cleanup(); vi.clearAllMocks(); });

it('keeps the call alive while the first hub entry saves, and ends it on an actual identity switch', () => {
  const props: MainLayoutProps = {
    identityId: 'current-identity', activeView: 'chat', setActiveView: vi.fn(),
    activeServer: null, servers: [], serverImageUrl: null, canModerate: true,
    permissionsStatus: 'ready', loadPermissions: vi.fn(async () => {}),
    pendingProtocolInviteConfirmation: null, handleAcceptProtocolInvite: vi.fn(async () => {}),
    handleIgnoreProtocolInvite: vi.fn(), degradedStartup: null,
    setDegradedStartup: vi.fn(), reconnectionBanner: null,
  };
  const view = render(<MainLayout {...props} />);
  const server = { id: 'saved-server', identityId: props.identityId, label: 'Annex', slug: 'current' } as SavedServer;
  view.rerender(<MainLayout {...props} activeServer={server} servers={[server]} />);
  expect(lifetime.start).toHaveBeenCalledOnce();
  expect(lifetime.stop).not.toHaveBeenCalled();
  view.rerender(<MainLayout {...props} identityId="another-server-identity" activeServer={server} />);
  expect(lifetime.stop).toHaveBeenCalledOnce();
  expect(lifetime.start).toHaveBeenCalledTimes(2);
});
