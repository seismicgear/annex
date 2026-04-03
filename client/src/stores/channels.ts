/**
 * Channel store — manages channel list, active channel, and messages.
 */

import { create } from 'zustand';
import type { Channel, Message, WsReceiveFrame } from '@/types';
import * as api from '@/lib/api';
import { AnnexWebSocket } from '@/lib/ws';
import { useVoiceStore } from '@/stores/voice';

/** Number of messages per pagination page. */
const PAGE_SIZE = 50;

/** A message that has been sent to the server but not yet acknowledged. */
export interface PendingSend {
  /** Client-generated request ID sent in the WS frame. */
  clientRequestId: string;
  /** The message content (for restoring the composer on failure). */
  content: string;
  /** Timestamp when the send was initiated. */
  sentAt: number;
}

interface ChannelsState {
  /** All available channels. */
  channels: Channel[];
  /** Currently selected channel ID. */
  activeChannelId: string | null;
  /** Set of channel IDs the user is currently a member of. */
  joinedChannelIds: Set<string>;
  /** Messages for the active channel (newest last). */
  messages: Message[];
  /** Whether the WebSocket is connected. */
  wsConnected: boolean;
  /** Loading state for channel list. */
  loading: boolean;
  /** Error message from loading channels. */
  error: string | null;
  /** Error message from send/edit/delete operations (shown inline near composer). */
  composerError: string | null;
  /** Whether channel history is currently loading. */
  historyLoading: boolean;
  /** Error message from loading channel history (distinct from channel list error). */
  historyError: string | null;
  /** Whether older messages are currently being fetched. */
  loadingOlder: boolean;
  /** Whether there are more older messages to load. */
  hasMoreMessages: boolean;
  /** The WebSocket instance (internal). */
  ws: AnnexWebSocket | null;
  /** Messages awaiting server acknowledgement, keyed by clientRequestId. */
  pendingSends: Map<string, PendingSend>;

  /** Load channel list from server. */
  loadChannels: (pseudonymId: string) => Promise<void>;
  /** Select a channel and load its history. */
  selectChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Connect WebSocket for real-time messages. Optional baseUrl for cross-server. */
  connectWs: (pseudonymId: string, baseUrl?: string, sessionToken?: string | null) => void;
  /** Send a message to the active channel. Returns the client request ID if queued, or null on failure. */
  sendMessage: (content: string, replyTo?: string | null) => string | null;
  /** Resolve a pending send (called when server echo or error arrives). */
  resolvePendingSend: (clientRequestId: string) => PendingSend | undefined;
  /** Get the pending send for a given request ID. */
  getPendingSend: (clientRequestId: string) => PendingSend | undefined;
  /** Edit a message in the active channel. */
  editMessage: (messageId: string, content: string) => void;
  /** Delete a message in the active channel. */
  deleteMessage: (messageId: string) => void;
  /** Load older messages (pagination). */
  loadOlderMessages: (pseudonymId: string) => Promise<void>;
  /** Create a new channel. */
  createChannel: (pseudonymId: string, name: string, channelType: string, topic?: string, federated?: boolean) => Promise<void>;
  /** Join a channel. */
  joinChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Leave a channel. */
  leaveChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Clear the composer error (on successful send, channel switch, or dismissal). */
  clearComposerError: () => void;
  /** Disconnect WebSocket. */
  disconnectWs: () => void;
  /** Update the active WebSocket's session token (for reconnect freshness). */
  updateWsSessionToken: (token: string | null) => void;
  /** Reset all per-server transient state to initial values. */
  resetServerState: () => void;
}

export const useChannelsStore = create<ChannelsState>((set, get) => ({
  channels: [],
  activeChannelId: null,
  joinedChannelIds: new Set<string>(),
  messages: [],
  wsConnected: false,
  loading: false,
  error: null,
  composerError: null,
  historyLoading: false,
  historyError: null,
  loadingOlder: false,
  hasMoreMessages: true,
  ws: null,
  pendingSends: new Map<string, PendingSend>(),

  loadChannels: async (pseudonymId: string) => {
    set({ loading: true, error: null });
    try {
      const channels = await api.listChannels(pseudonymId);
      // Build joined set from is_member flag if available
      const joined = new Set<string>();
      for (const ch of channels) {
        if (ch.is_member) joined.add(ch.channel_id);
      }
      set({ channels, joinedChannelIds: joined });
    } catch (err) {
      set({ channels: [], error: err instanceof Error ? err.message : String(err) });
    } finally {
      set({ loading: false });
    }
  },

  selectChannel: async (pseudonymId: string, channelId: string) => {
    const { ws, activeChannelId: prevChannelId } = get();

    // Unsubscribe from the previous channel's real-time updates.
    if (ws && prevChannelId && prevChannelId !== channelId) {
      ws.unsubscribe(prevChannelId);
    }

    set({ activeChannelId: channelId, messages: [], loadingOlder: false, hasMoreMessages: true, historyLoading: true, historyError: null, composerError: null });

    // Auto-join the channel (idempotent — no-op if already a member).
    // Must be a member before fetching messages or joining voice.
    try {
      await api.joinChannel(pseudonymId, channelId);
      set((s) => {
        const joined = new Set(s.joinedChannelIds);
        joined.add(channelId);
        return { joinedChannelIds: joined };
      });
    } catch {
      // May fail for capability restrictions; still try to load messages
      // in case the user is already a member from a previous session.
    }

    // Subscribe to real-time updates for this channel via WebSocket.
    if (ws) {
      ws.subscribe(channelId);
    }

    try {
      const messages = await api.getMessages(pseudonymId, channelId, undefined, PAGE_SIZE);
      set({ messages: messages.reverse(), hasMoreMessages: messages.length >= PAGE_SIZE, historyLoading: false, historyError: null });
    } catch (err) {
      // Keep the channel selected but surface the history error so the
      // message pane can show a retry affordance instead of staying blank.
      set({
        historyLoading: false,
        historyError: err instanceof Error ? err.message : 'Failed to load channel history',
      });
    }
  },

  connectWs: (pseudonymId: string, baseUrl?: string, sessionToken?: string | null) => {
    const existing = get().ws;
    if (existing) existing.disconnect();

    const ws = new AnnexWebSocket(pseudonymId, baseUrl, sessionToken ?? null);

    ws.onStatus((connected) => set({ wsConnected: connected }));

    ws.onMessage((frame: WsReceiveFrame) => {
      // Handle error frames — route to composerError for chat-flow errors.
      // If the error carries a clientRequestId, resolve the pending send
      // so the composer can restore the draft.
      if (frame.type === 'error') {
        const errorMsg = frame.message ?? frame.error ?? 'Unknown WebSocket error';
        if (frame.clientRequestId) {
          get().resolvePendingSend(frame.clientRequestId);
        }
        set({ composerError: errorMsg });
        return;
      }

      if (frame.channelId !== get().activeChannelId) return;

      if (frame.type === 'message') {
        // Resolve the pending send when the server echoes our message back.
        if (frame.clientRequestId) {
          get().resolvePendingSend(frame.clientRequestId);
        }
        const msg: Message = {
          message_id: frame.messageId ?? '',
          channel_id: frame.channelId,
          sender_pseudonym: frame.senderPseudonym ?? '',
          content: frame.content ?? '',
          reply_to_message_id: frame.replyToMessageId ?? null,
          created_at: frame.createdAt ?? new Date().toISOString(),
          edited_at: frame.editedAt ?? null,
          deleted_at: frame.deletedAt ?? null,
        };
        set((state) => ({ messages: [...state.messages, msg] }));
      } else if (frame.type === 'message_edited') {
        set((state) => ({
          messages: state.messages.map((m) =>
            m.message_id === frame.messageId
              ? { ...m, content: frame.content ?? m.content, edited_at: frame.editedAt ?? null }
              : m,
          ),
        }));
      } else if (frame.type === 'message_deleted') {
        set((state) => ({
          messages: state.messages.map((m) =>
            m.message_id === frame.messageId
              ? { ...m, content: '', deleted_at: frame.deletedAt ?? null }
              : m,
          ),
        }));
      }
    });

    ws.connect();
    set({ ws });
  },

  sendMessage: (content: string, replyTo: string | null = null): string | null => {
    const { ws, activeChannelId } = get();
    if (!ws || !activeChannelId) {
      set({ composerError: 'Cannot send — not connected to the server.' });
      return null;
    }
    set({ composerError: null });
    try {
      const clientRequestId = ws.send(activeChannelId, content, replyTo);
      const pending: PendingSend = { clientRequestId, content, sentAt: Date.now() };
      set((s) => {
        const next = new Map(s.pendingSends);
        next.set(clientRequestId, pending);
        return { pendingSends: next };
      });
      return clientRequestId;
    } catch (err) {
      console.error('[channels] sendMessage threw:', err);
      set({ composerError: err instanceof Error ? err.message : 'Failed to send message' });
      return null;
    }
  },

  resolvePendingSend: (clientRequestId: string): PendingSend | undefined => {
    const { pendingSends } = get();
    const pending = pendingSends.get(clientRequestId);
    if (pending) {
      set((s) => {
        const next = new Map(s.pendingSends);
        next.delete(clientRequestId);
        return { pendingSends: next };
      });
    }
    return pending;
  },

  getPendingSend: (clientRequestId: string): PendingSend | undefined => {
    return get().pendingSends.get(clientRequestId);
  },

  editMessage: (messageId: string, content: string) => {
    const { ws, activeChannelId } = get();
    if (!ws || !activeChannelId) return;
    try {
      ws.editMessage(activeChannelId, messageId, content);
    } catch (err) {
      console.error('[channels] editMessage threw:', err);
    }
  },

  deleteMessage: (messageId: string) => {
    const { ws, activeChannelId } = get();
    if (!ws || !activeChannelId) return;
    try {
      ws.deleteMessage(activeChannelId, messageId);
    } catch (err) {
      console.error('[channels] deleteMessage threw:', err);
    }
  },

  loadOlderMessages: async (pseudonymId: string) => {
    const { activeChannelId, messages, loadingOlder, hasMoreMessages } = get();
    if (!activeChannelId || messages.length === 0 || loadingOlder || !hasMoreMessages) return;

    set({ loadingOlder: true });
    try {
      const oldest = messages[0];
      const older = await api.getMessages(pseudonymId, activeChannelId, oldest.message_id, PAGE_SIZE);
      set((state) => ({
        messages: [...older.reverse(), ...state.messages],
        hasMoreMessages: older.length >= PAGE_SIZE,
      }));
    } catch (err) {
      console.warn('[channels] loadOlderMessages failed:', err);
      // Stop trying to load more if the request failed to prevent
      // infinite retry loops on scroll.
      set({ hasMoreMessages: false });
    } finally {
      set({ loadingOlder: false });
    }
  },

  createChannel: async (pseudonymId, name, channelType, topic, federated) => {
    // The server returns {"status": "created"}, not a Channel object,
    // so we don't optimistically add to the list. The caller should
    // call loadChannels() after to refresh the full list.
    await api.createChannel(pseudonymId, name, channelType, topic, federated);
  },

  joinChannel: async (pseudonymId, channelId) => {
    await api.joinChannel(pseudonymId, channelId);
    set((s) => {
      const joined = new Set(s.joinedChannelIds);
      joined.add(channelId);
      return { joinedChannelIds: joined };
    });
  },

  leaveChannel: async (pseudonymId, channelId) => {
    await api.leaveChannel(pseudonymId, channelId);

    set((s) => {
      const joined = new Set(s.joinedChannelIds);
      joined.delete(channelId);
      return { joinedChannelIds: joined };
    });

    // If the user is in a voice call on this channel, leave it too.
    const voiceStore = useVoiceStore.getState();
    if (voiceStore.connectedChannelId === channelId) {
      try {
        await voiceStore.leaveCall(pseudonymId);
      } catch { /* best effort */ }
      voiceStore.forceReset();
    }
    voiceStore.clearChannelCallState(channelId);

    const { activeChannelId } = get();
    if (activeChannelId === channelId) {
      set({ activeChannelId: null, messages: [] });
    }
  },

  clearComposerError: () => {
    set({ composerError: null });
  },

  disconnectWs: () => {
    const { ws } = get();
    if (ws) {
      ws.disconnect();
      set({ ws: null, wsConnected: false });
    }
  },

  updateWsSessionToken: (token: string | null) => {
    const { ws } = get();
    if (ws) {
      ws.setSessionToken(token);
    }
  },

  resetServerState: () => {
    const { ws } = get();
    if (ws) {
      ws.disconnect();
    }
    set({
      channels: [],
      activeChannelId: null,
      joinedChannelIds: new Set<string>(),
      messages: [],
      wsConnected: false,
      loading: false,
      error: null,
      composerError: null,
      historyLoading: false,
      historyError: null,
      loadingOlder: false,
      hasMoreMessages: true,
      ws: null,
      pendingSends: new Map<string, PendingSend>(),
    });
  },
}));
