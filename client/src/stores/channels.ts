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

/** Maximum messages to keep in the rendered window to prevent unbounded memory growth. */
const MAX_MESSAGE_WINDOW = 200;

/** Typing indicator timeout in milliseconds. */
const TYPING_TIMEOUT_MS = 5_000;

/** Minimum interval between sending typing frames (debounce). */
const TYPING_DEBOUNCE_MS = 3_000;

/** A message that has been sent to the server but not yet acknowledged. */
export interface PendingSend {
  /** Client-generated request ID sent in the WS frame. */
  clientRequestId: string;
  /** The message content (for restoring the composer on failure). */
  content: string;
  /** Timestamp when the send was initiated. */
  sentAt: number;
}

/** Typing state for a channel. */
interface TypingUser {
  pseudonymId: string;
  /** Timestamp when the typing indicator was last received. */
  lastTypedAt: number;
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
  /** Users currently typing in the active channel. */
  typingUsers: TypingUser[];
  /** Unread message count per channel. */
  unreadCounts: Record<string, number>;
  /** Last read message ID per channel. */
  lastReadMessageIds: Record<string, string>;
  /** Message being replied to (shown in composer). */
  replyToMessage: Message | null;

  /** Load channel list from server. */
  loadChannels: (pseudonymId: string) => Promise<void>;
  /** Select a channel and load its history. */
  selectChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Connect WebSocket for real-time messages. Optional baseUrl for cross-server. */
  connectWs: (pseudonymId: string, baseUrl?: string, sessionToken?: string | null) => void;
  /** Send a message to the active channel. Returns the client request ID if queued, or null on failure. */
  sendMessage: (content: string, pseudonymId: string, replyTo?: string | null) => string | null;
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
  /** Send a typing indicator for the active channel (debounced). */
  sendTyping: () => void;
  /** Mark the active channel as read (updates lastReadMessageIds). */
  markChannelRead: (channelId: string) => void;
  /** Retry a failed optimistic message. */
  retryMessage: (clientRequestId: string, pseudonymId: string) => void;
  /** Dismiss a failed optimistic message. */
  dismissFailedMessage: (clientRequestId: string) => void;
  /** Set the message to reply to. */
  setReplyTo: (message: Message | null) => void;
}

/** Timestamp of the last typing frame sent (module-level to survive store resets). */
let lastTypingSentAt = 0;

/** Typing cleanup interval handle. */
let typingCleanupInterval: ReturnType<typeof setInterval> | null = null;

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
  typingUsers: [],
  unreadCounts: {},
  lastReadMessageIds: {},
  replyToMessage: null,

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

    set({ activeChannelId: channelId, messages: [], loadingOlder: false, hasMoreMessages: true, historyLoading: true, historyError: null, composerError: null, typingUsers: [], replyToMessage: null });

    // Mark the channel as read
    get().markChannelRead(channelId);

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

    // Fetch history BEFORE subscribing to WS to avoid a race where
    // messages arriving between subscribe() and getMessages() completion
    // are silently dropped when the history response replaces the array.
    try {
      const messages = await api.getMessages(pseudonymId, channelId, undefined, PAGE_SIZE);
      const reversed = messages.reverse();
      set({ messages: reversed, hasMoreMessages: messages.length >= PAGE_SIZE, historyLoading: false, historyError: null });
      // Track the newest message ID for resume
      if (reversed.length > 0 && ws) {
        ws.trackLastMessageId(channelId, reversed[reversed.length - 1].message_id);
      }
    } catch (err) {
      // Keep the channel selected but surface the history error so the
      // message pane can show a retry affordance instead of staying blank.
      set({
        historyLoading: false,
        historyError: err instanceof Error ? err.message : 'Failed to load channel history',
      });
    }

    // Subscribe to real-time updates AFTER history is loaded so no
    // messages are lost in the gap.
    if (ws) {
      ws.subscribe(channelId);
    }
  },

  connectWs: (pseudonymId: string, baseUrl?: string, sessionToken?: string | null) => {
    const existing = get().ws;
    if (existing) existing.disconnect();

    // Clear typing cleanup interval
    if (typingCleanupInterval) {
      clearInterval(typingCleanupInterval);
      typingCleanupInterval = null;
    }

    const ws = new AnnexWebSocket(pseudonymId, baseUrl, sessionToken ?? null);

    ws.onStatus((connected) => set({ wsConnected: connected }));

    ws.onMessage((frame: WsReceiveFrame) => {
      // Handle error frames — route to composerError for chat-flow errors.
      // If the error carries a clientRequestId, resolve the pending send
      // and mark the optimistic message as failed.
      if (frame.type === 'error') {
        const errorMsg = frame.message ?? frame.error ?? 'Unknown WebSocket error';
        if (frame.clientRequestId) {
          get().resolvePendingSend(frame.clientRequestId);
          // Mark the optimistic message as failed
          set((state) => ({
            messages: state.messages.map((m) =>
              m.clientRequestId === frame.clientRequestId
                ? { ...m, pending: false, failed: true }
                : m,
            ),
            composerError: errorMsg,
          }));
        } else {
          set({ composerError: errorMsg });
        }
        return;
      }

      // Handle channel lifecycle events (server-wide, not per-channel)
      if (frame.type === 'channel_created' && frame.channel) {
        set((state) => {
          // Avoid duplicates
          if (state.channels.some((c) => c.channel_id === frame.channel!.channel_id)) {
            return state;
          }
          return { channels: [...state.channels, frame.channel!] };
        });
        return;
      }
      if (frame.type === 'channel_deleted' && frame.channelId) {
        set((state) => ({
          channels: state.channels.filter((c) => c.channel_id !== frame.channelId),
        }));
        // If the deleted channel was active, clear the view
        if (get().activeChannelId === frame.channelId) {
          set({ activeChannelId: null, messages: [], typingUsers: [] });
        }
        return;
      }

      // Handle typing indicators
      if (frame.type === 'typing' && frame.pseudonymId && frame.channelId) {
        // Ignore own typing echoes
        if (frame.pseudonymId === pseudonymId) return;
        // Only show typing for active channel
        if (frame.channelId !== get().activeChannelId) return;
        set((state) => {
          const now = Date.now();
          const existing = state.typingUsers.find((u) => u.pseudonymId === frame.pseudonymId);
          if (existing) {
            return { typingUsers: state.typingUsers.map((u) => u.pseudonymId === frame.pseudonymId ? { ...u, lastTypedAt: now } : u) };
          }
          return { typingUsers: [...state.typingUsers, { pseudonymId: frame.pseudonymId!, lastTypedAt: now }] };
        });
        return;
      }

      // Handle resume acknowledgement
      if (frame.type === 'resumed') {
        // Resume complete — no action needed, messages were already processed as normal frames
        return;
      }

      if (frame.channelId !== get().activeChannelId) {
        // Increment unread count for non-active channels
        if (frame.type === 'message' && frame.channelId) {
          set((state) => ({
            unreadCounts: {
              ...state.unreadCounts,
              [frame.channelId!]: (state.unreadCounts[frame.channelId!] ?? 0) + 1,
            },
          }));
          // Browser notification for background messages
          if (document.hidden && 'Notification' in globalThis && Notification.permission === 'granted') {
            const sender = frame.senderPseudonym?.slice(0, 12) ?? 'Someone';
            const body = (frame.content ?? '').slice(0, 100);
            try {
              new Notification(`${sender}...`, { body, tag: `annex-${frame.channelId}` });
            } catch { /* Notification API unavailable */ }
          }
        }
        return;
      }

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
        set((state) => {
          // If this message confirms an optimistic send, replace it
          if (frame.clientRequestId) {
            const hasOptimistic = state.messages.some((m) => m.clientRequestId === frame.clientRequestId);
            if (hasOptimistic) {
              const updated = state.messages.map((m) =>
                m.clientRequestId === frame.clientRequestId
                  ? { ...msg } // Replace optimistic with confirmed
                  : m,
              );
              return { messages: updated };
            }
          }
          // Deduplicate: skip if a message with the same ID already exists
          if (msg.message_id && state.messages.some((m) => m.message_id === msg.message_id)) {
            return state;
          }
          let messages = [...state.messages, msg];
          // Trim to sliding window — remove oldest when exceeding cap
          if (messages.length > MAX_MESSAGE_WINDOW) {
            messages = messages.slice(messages.length - MAX_MESSAGE_WINDOW);
          }
          return { messages };
        });
        // Track last message ID for resume
        if (msg.message_id && ws) {
          ws.trackLastMessageId(frame.channelId!, msg.message_id);
        }
        // Clear typing indicator for the sender
        if (frame.senderPseudonym) {
          set((state) => ({
            typingUsers: state.typingUsers.filter((u) => u.pseudonymId !== frame.senderPseudonym),
          }));
        }
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

    // Start typing cleanup interval — remove stale typing indicators every second
    typingCleanupInterval = setInterval(() => {
      const now = Date.now();
      set((state) => {
        const filtered = state.typingUsers.filter((u) => now - u.lastTypedAt < TYPING_TIMEOUT_MS);
        if (filtered.length !== state.typingUsers.length) {
          return { typingUsers: filtered };
        }
        return state;
      });
    }, 1000);
  },

  sendMessage: (content: string, pseudonymId: string, replyTo: string | null = null): string | null => {
    const { ws, activeChannelId, replyToMessage } = get();
    if (!ws || !activeChannelId) {
      set({ composerError: 'Cannot send — not connected to the server.' });
      return null;
    }
    // Use the reply context if set and no explicit replyTo was passed
    const effectiveReplyTo = replyTo ?? replyToMessage?.message_id ?? null;
    set({ composerError: null, replyToMessage: null });
    try {
      const clientRequestId = ws.send(activeChannelId, content, effectiveReplyTo);
      const pending: PendingSend = { clientRequestId, content, sentAt: Date.now() };
      // Add optimistic message to the list immediately
      const optimisticMsg: Message = {
        message_id: '', // Will be assigned by server
        channel_id: activeChannelId,
        sender_pseudonym: pseudonymId,
        content,
        reply_to_message_id: effectiveReplyTo,
        created_at: new Date().toISOString(),
        pending: true,
        clientRequestId,
      };
      set((s) => {
        const next = new Map(s.pendingSends);
        next.set(clientRequestId, pending);
        return { pendingSends: next, messages: [...s.messages, optimisticMsg] };
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
      // Find the oldest non-pending message for cursor
      const oldest = messages.find((m) => !m.pending && !m.failed);
      if (!oldest) { set({ loadingOlder: false }); return; }
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
    await api.createChannel(pseudonymId, name, channelType, topic, federated);
    // Channel list will be updated via WS channel_created event
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
      set({ activeChannelId: null, messages: [], typingUsers: [] });
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
    if (typingCleanupInterval) {
      clearInterval(typingCleanupInterval);
      typingCleanupInterval = null;
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
    if (typingCleanupInterval) {
      clearInterval(typingCleanupInterval);
      typingCleanupInterval = null;
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
      typingUsers: [],
      unreadCounts: {},
      lastReadMessageIds: {},
    });
  },

  sendTyping: () => {
    const { ws, activeChannelId } = get();
    if (!ws || !activeChannelId) return;
    const now = Date.now();
    if (now - lastTypingSentAt < TYPING_DEBOUNCE_MS) return;
    lastTypingSentAt = now;
    ws.sendTyping(activeChannelId);
  },

  markChannelRead: (channelId: string) => {
    const { messages } = get();
    const lastMsg = messages[messages.length - 1];
    set((state) => ({
      unreadCounts: { ...state.unreadCounts, [channelId]: 0 },
      lastReadMessageIds: lastMsg
        ? { ...state.lastReadMessageIds, [channelId]: lastMsg.message_id }
        : state.lastReadMessageIds,
    }));
  },

  retryMessage: (clientRequestId: string, pseudonymId: string) => {
    const { messages, ws, activeChannelId } = get();
    const failedMsg = messages.find((m) => m.clientRequestId === clientRequestId && m.failed);
    if (!failedMsg || !ws || !activeChannelId) return;

    // Remove the failed message and re-send
    set((state) => ({
      messages: state.messages.filter((m) => m.clientRequestId !== clientRequestId),
      composerError: null,
    }));
    get().sendMessage(failedMsg.content, pseudonymId, failedMsg.reply_to_message_id);
  },

  dismissFailedMessage: (clientRequestId: string) => {
    set((state) => ({
      messages: state.messages.filter((m) => m.clientRequestId !== clientRequestId),
    }));
  },

  setReplyTo: (message: Message | null) => {
    set({ replyToMessage: message });
  },
}));
