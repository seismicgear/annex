/**
 * Servers store — manages the client-side node hub.
 *
 * The server list is a rendering of the user's local database of established
 * Merkle tree insertions. Switching servers triggers an immediate UI transition
 * while the cryptographic handshake runs in the background.
 */

import { create } from 'zustand';
import type { SavedServer, ServerSummary } from '@/types';
import * as serversDb from '@/lib/servers';
import * as api from '@/lib/api';
import { useIdentityStore } from './identity';
import { useChannelsStore } from './channels';

interface ServersState {
  /** All saved server connections. */
  servers: SavedServer[];
  /** Currently active server ID (null = current origin). */
  activeServerId: string | null;
  /** Whether a server switch is in progress. */
  switching: boolean;

  /** Load saved servers from IndexedDB. */
  loadServers: () => Promise<void>;
  /** Switch to a different server context. Immediate UI + async crypto. */
  switchServer: (serverId: string) => Promise<void>;
  /** Register the current origin as a saved server for an identity. */
  saveCurrentServer: (identityId: string, slug: string, label: string) => Promise<void>;
  /** Add a remote server via federation hopping. Returns the new server ID. */
  addRemoteServer: (baseUrl: string) => Promise<SavedServer | null>;
  /** Remove a saved server. */
  removeServer: (serverId: string) => Promise<void>;
  /** Update persona mapping for a server. */
  setServerPersona: (serverId: string, personaId: string | null, accentColor?: string) => Promise<void>;
  /** Get the active server entry. */
  getActiveServer: () => SavedServer | null;
}

export const useServersStore = create<ServersState>((set, get) => ({
  servers: [],
  activeServerId: null,
  switching: false,

  loadServers: async () => {
    const servers = await serversDb.listServers();
    set({ servers });
  },

  switchServer: async (serverId: string) => {
    const { servers, activeServerId } = get();
    if (serverId === activeServerId) return;

    const server = servers.find((s) => s.id === serverId);
    if (!server) return;

    // Immediate: update active server for instant UI transition
    set({ activeServerId: serverId, switching: true });

    try {
      // Set API base URL for cross-server requests
      api.setApiBaseUrl(server.baseUrl);

      // Switch identity context
      const identityStore = useIdentityStore.getState();
      await identityStore.selectIdentity(server.identityId);

      const identity = useIdentityStore.getState().identity;
      if (!identity?.pseudonymId) {
        set({ switching: false });
        return;
      }

      // Reconnect WebSocket to the target server
      const channelsStore = useChannelsStore.getState();
      channelsStore.disconnectWs();
      channelsStore.connectWs(identity.pseudonymId, server.baseUrl);

      // Load channels and permissions for the new server
      await channelsStore.loadChannels(identity.pseudonymId);
      await identityStore.loadPermissions();

      // Update last connected timestamp
      server.lastConnectedAt = new Date().toISOString();
      await serversDb.saveServer(server);

      // Refresh cached summary in background (failures are non-fatal;
      // the stale cached summary remains until the next successful fetch)
      api.getServerSummary()
        .then((summary) => serversDb.updateCachedSummary(serverId, summary))
        .catch(() => { /* stale summary retained */ });

    } finally {
      set({ switching: false });
    }
  },

  saveCurrentServer: async (identityId: string, slug: string, label: string) => {
    // Check if already saved
    const existing = await serversDb.getServerByIdentityId(identityId);
    if (existing) {
      set((state) => ({ activeServerId: existing.id, servers: state.servers }));
      return;
    }

    const server = serversDb.createServerEntry('', slug, label, identityId);

    // Try to fetch and cache the server summary
    try {
      const summary = await api.getServerSummary();
      server.cachedSummary = summary;
      server.label = summary.label || label;
    } catch {
      // Non-fatal: server summary unavailable; label falls back to slug
    }

    await serversDb.saveServer(server);
    const servers = await serversDb.listServers();
    set({ servers, activeServerId: server.id });
  },

  addRemoteServer: async (baseUrl: string) => {
    // Check if we already have this server
    const { servers } = get();
    const existing = servers.find((s) => s.baseUrl === baseUrl);
    if (existing) return existing;

    try {
      // Fetch the remote server's public summary
      const summary: ServerSummary = await api.getRemoteServerSummary(baseUrl);

      // Create a placeholder server entry (identity will be created during switch)
      const server = serversDb.createServerEntry(
        baseUrl,
        summary.slug,
        summary.label,
        '', // identityId will be set after registration
      );
      server.cachedSummary = summary;

      await serversDb.saveServer(server);
      const allServers = await serversDb.listServers();
      set({ servers: allServers });

      return server;
    } catch {
      return null;
    }
  },

  removeServer: async (serverId: string) => {
    const { activeServerId } = get();
    await serversDb.removeServer(serverId);
    const servers = await serversDb.listServers();

    if (activeServerId === serverId) {
      // Switch back to the first available server or null
      const fallback = servers[0] ?? null;
      set({ servers, activeServerId: fallback?.id ?? null });

      if (fallback) {
        // Re-connect to fallback server
        get().switchServer(fallback.id);
      } else {
        // No servers left — reset to current origin
        api.setApiBaseUrl('');
      }
    } else {
      set({ servers });
    }
  },

  setServerPersona: async (serverId: string, personaId: string | null, accentColor?: string) => {
    const { servers } = get();
    const server = servers.find((s) => s.id === serverId);
    if (!server) return;

    server.personaId = personaId;
    if (accentColor) server.accentColor = accentColor;
    await serversDb.saveServer(server);

    set({ servers: [...servers] });
  },

  getActiveServer: () => {
    const { servers, activeServerId } = get();
    return servers.find((s) => s.id === activeServerId) ?? null;
  },
}));
