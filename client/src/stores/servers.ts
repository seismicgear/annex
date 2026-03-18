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
import { useVoiceStore } from './voice';

interface ServersState {
  /** All saved server connections. */
  servers: SavedServer[];
  /** Currently active server ID (null = current origin). */
  activeServerId: string | null;
  /** Whether a server switch is in progress. */
  switching: boolean;
  /** Image URL for the active server (not persisted). */
  serverImageUrl: string | null;
  /** Server ID of the placeholder currently being registered (for App.tsx to track). */
  pendingRegistrationServerId: string | null;

  /** Load saved servers from IndexedDB. */
  loadServers: () => Promise<void>;
  /** Switch to a different server context. Immediate UI + async crypto. */
  switchServer: (serverId: string) => Promise<void>;
  /** Register the current server as a saved server for an identity. */
  saveCurrentServer: (identityId: string, slug: string, label: string, baseUrl?: string) => Promise<void>;
  /** Add a remote server via federation hopping. Returns the new server ID. */
  addRemoteServer: (baseUrl: string) => Promise<SavedServer | null>;
  /** Remove a saved server. */
  removeServer: (serverId: string) => Promise<void>;
  /** Update persona mapping for a server. */
  setServerPersona: (serverId: string, personaId: string | null, accentColor?: string) => Promise<void>;
  /** Get the active server entry. */
  getActiveServer: () => SavedServer | null;
  /** Set the active server's image URL (called after upload or fetch). */
  setServerImageUrl: (url: string | null) => void;
  /** Fetch and cache the active server's image URL. */
  fetchServerImage: () => Promise<void>;
  /** Find a saved server by its base URL. */
  findServerByBaseUrl: (baseUrl: string) => SavedServer | undefined;
  /** Fulfill a placeholder entry with the real identityId and make it active. */
  fulfillPlaceholder: (serverId: string, identityId: string, label?: string) => Promise<void>;
  /**
   * Begin remote registration: clone identity, add placeholder server,
   * set API target, and trigger the registration state machine.
   * Shared by ServerHub, FederationPanel, and protocol invite paths.
   */
  beginRemoteRegistration: (baseUrl: string) => Promise<SavedServer | null>;
}

export const useServersStore = create<ServersState>((set, get) => ({
  servers: [],
  activeServerId: null,
  switching: false,
  serverImageUrl: null,
  pendingRegistrationServerId: null,

  loadServers: async () => {
    const servers = await serversDb.listServers();
    set({ servers });
  },

  switchServer: async (serverId: string) => {
    const { servers, activeServerId } = get();
    if (serverId === activeServerId) return;

    const server = servers.find((s) => s.id === serverId);
    if (!server) return;

    // Guard: don't switch to a server that hasn't been registered yet
    if (!server.identityId) return;

    // Leave or forcibly clear any active voice session BEFORE changing
    // activeServerId. This prevents the old LiveKitRoom from being rendered
    // under the new server's state.
    const voiceStore = useVoiceStore.getState();
    if (voiceStore.connectedChannelId || voiceStore.voiceToken) {
      const oldIdentity = useIdentityStore.getState().identity;
      if (oldIdentity?.pseudonymId) {
        // Best-effort: try to leave gracefully on the old server
        await voiceStore.leaveCall(oldIdentity.pseudonymId).catch(() => {});
      }
      // Force-reset regardless of leaveCall result
      voiceStore.forceReset();
    }

    // Clear all per-server transient state before switching so the UI
    // never shows stale channels/messages from the previous server.
    const channelsStore = useChannelsStore.getState();
    channelsStore.resetServerState();

    // Immediate: update active server for instant UI transition.
    // Clear serverImageUrl so stale imagery from the previous server is never shown.
    set({ activeServerId: serverId, switching: true, serverImageUrl: null });

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

      // Reconnect WebSocket to the target server with the session token
      // for authenticated connections (matches the startup path in App.tsx).
      const sessionToken = identity.sessionToken ?? null;
      channelsStore.connectWs(identity.pseudonymId, server.baseUrl, sessionToken);

      // Load channels and permissions for the new server
      await channelsStore.loadChannels(identity.pseudonymId);
      await identityStore.loadPermissions();

      // Update last connected timestamp
      server.lastConnectedAt = new Date().toISOString();
      await serversDb.saveServer(server);

      // Refresh cached summary and server image in background
      api.getServerSummary()
        .then((summary) => serversDb.updateCachedSummary(serverId, summary))
        .catch(() => { /* stale summary retained */ });
      get().fetchServerImage();

    } finally {
      set({ switching: false });
    }
  },

  saveCurrentServer: async (identityId: string, slug: string, label: string, baseUrl?: string) => {
    const effectiveBaseUrl = baseUrl ?? '';

    // Check if already saved by identity
    const existingByIdentity = await serversDb.getServerByIdentityId(identityId);
    if (existingByIdentity) {
      set((state) => ({ activeServerId: existingByIdentity.id, servers: state.servers }));
      return;
    }

    // Look for a placeholder entry created by addRemoteServer() for this
    // same remote server (matching baseUrl or slug with empty identityId).
    // Update it in-place instead of creating a duplicate.
    let server: import('@/types').SavedServer | null = null;
    if (effectiveBaseUrl) {
      const { servers } = get();
      const placeholder = servers.find(
        (s) => s.identityId === '' && (s.baseUrl === effectiveBaseUrl || s.slug === slug),
      );
      if (placeholder) {
        placeholder.identityId = identityId;
        placeholder.label = label;
        placeholder.baseUrl = effectiveBaseUrl;
        placeholder.lastConnectedAt = new Date().toISOString();
        server = placeholder;
      }
    }

    if (!server) {
      server = serversDb.createServerEntry(effectiveBaseUrl, slug, label, identityId);
    }

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
      const fallback = servers[0] ?? null;
      if (fallback) {
        // Update the server list but let switchServer handle activeServerId.
        // Setting activeServerId here would cause switchServer to short-circuit
        // (same-server guard) and skip the full reconnect path.
        set({ servers });
        await get().switchServer(fallback.id);
      } else {
        // No servers left — reset to current origin
        set({ servers, activeServerId: null });
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

  setServerImageUrl: (url: string | null) => {
    set({ serverImageUrl: url });
  },

  fetchServerImage: async () => {
    try {
      const resp = await api.getServerImage();
      set({ serverImageUrl: resp.image_url ? api.resolveUrl(resp.image_url) : null });
    } catch {
      // Fetch failed — clear any stale image from the previous server
      set({ serverImageUrl: null });
    }
  },

  findServerByBaseUrl: (baseUrl: string) => {
    const { servers } = get();
    return servers.find((s) => s.baseUrl === baseUrl);
  },

  fulfillPlaceholder: async (serverId: string, identityId: string, label?: string) => {
    const { servers } = get();
    const server = servers.find((s) => s.id === serverId);
    if (!server) return;

    server.identityId = identityId;
    server.lastConnectedAt = new Date().toISOString();
    if (label) server.label = label;

    // Try to refresh cached summary
    try {
      const summary = await api.getServerSummary();
      server.cachedSummary = summary;
      server.label = summary.label || server.label;
    } catch {
      // Non-fatal
    }

    await serversDb.saveServer(server);
    const allServers = await serversDb.listServers();
    set({ servers: allServers, activeServerId: serverId, pendingRegistrationServerId: null });
  },

  beginRemoteRegistration: async (baseUrl: string) => {
    const identityStore = useIdentityStore.getState();

    // 1. Add remote server placeholder (or return existing)
    const server = await get().addRemoteServer(baseUrl);
    if (!server) return null;

    // If the server already has an identity, just switch to it
    if (server.identityId) {
      await get().switchServer(server.id);
      return server;
    }

    // 2. Clone or derive a new identity record for this server
    const clonedId = await identityStore.cloneForServer();
    if (!clonedId) return null;

    // 3. Select the new identity so registration uses it (not the current one)
    await identityStore.selectIdentity(clonedId);

    // 4. Record which placeholder is being fulfilled
    set({ pendingRegistrationServerId: server.id });

    // 5. Set API base URL for the target server
    api.setApiBaseUrl(baseUrl);

    // 6. Reset identity phase to 'keys_ready' so the auto-register effect fires
    useIdentityStore.setState({
      phase: 'keys_ready',
      proofInFlight: false,
      provingStatus: 'idle',
      error: null,
      errorDetails: null,
    });

    return server;
  },
}));
