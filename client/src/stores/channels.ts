/**
 * Channel store — manages channel list, active channel, and messages.
 */

import { create } from 'zustand';
import type { Channel, Message, WsReceiveFrame } from '@/types';
import * as api from '@/lib/api';
import { AnnexWebSocket } from '@/lib/ws';
import { useVoiceStore } from '@/stores/voice';
import {
  decryptForDisplay,
  encryptForWire,
  ensureChannelReady,
  getChannelKeyError,
  getChannelKeyState,
  isChannelE2e,
  isChannelE2eUnknown,
  markChannelE2eUnknown,
  isE2eBody,
  markChannelE2e,
  resetE2eChannels,
  setE2eIdentity,
  type ChannelKeyState,
} from '@/lib/message-crypto';
import { clearE2eManagers } from '@/lib/e2e-store';

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
  /** Whether the active channel is end-to-end encrypted (reactive for UI). */
  activeChannelE2e: boolean;
  /** Re-attempt resolving the active channel's key, and reload if it lands. */
  retryChannelKey: (pseudonymId: string) => Promise<void>;
  /**
   * Whether this client can actually read the active E2E channel. `pending`
   * means the channel is keyed but we have not been admitted yet; `failed`
   * means resolving the key went wrong. Both used to be invisible, so a
   * channel full of "🔒 encrypted message (no key)" carried no explanation.
   */
  activeChannelKeyState: ChannelKeyState;
  /** Why the key could not be resolved, for `activeChannelKeyState === 'failed'`. */
  activeChannelKeyError: string | null;
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
  /** True while intentionally reconnecting WS to apply a refreshed auth token. */
  wsAuthRefreshing: boolean;
  /** Whether channel history is currently loading. */
  historyLoading: boolean;
  /** Error message from loading channel history (distinct from channel list error). */
  historyError: string | null;
  /** Whether older messages are currently being fetched. */
  loadingOlder: boolean;
  /** Whether there are more older messages to load. */
  hasMoreMessages: boolean;
  /**
   * Set when a scrollback page failed to load, so the UI can offer a retry.
   *
   * Kept separate from `hasMoreMessages` on purpose. A failed page used to
   * set `hasMoreMessages: false` to stop the scroll handler retrying in a
   * loop — which worked, and which also told the user they had reached the
   * beginning of the channel. One dropped request ended the history of that
   * channel for the rest of the session, and looked exactly like a short
   * channel.
   */
  olderError: string | null;
  /** Set when an edit could not be sent, so the user is not shown a false save. */
  editError: string | null;
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
  /** Last typing-frame send timestamp per channel. */
  typingSentAtByChannel: Record<string, number>;

  /** Load channel list from server. */
  loadChannels: (pseudonymId: string) => Promise<void>;
  /** Select a channel and load its history. */
  selectChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Turn on end-to-end encryption for the active channel (moderators only). */
  enableChannelE2e: (pseudonymId: string) => Promise<void>;
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
  /** Dismiss the "edit not saved" notice. */
  clearEditError: () => void;
  /** Retry a scrollback page that failed, clearing the error first. */
  retryOlderMessages: (pseudonymId: string) => Promise<void>;
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
  /** Mark a channel as read (updates lastReadMessageIds). */
  markChannelRead: (channelId: string, lastMessageId?: string) => void;
  /** Retry a failed optimistic message. */
  retryMessage: (clientRequestId: string, pseudonymId: string) => void;
  /** Dismiss a failed optimistic message. */
  dismissFailedMessage: (clientRequestId: string) => void;
  /** Set the message to reply to. */
  setReplyTo: (message: Message | null) => void;
}

/** Typing cleanup interval handle. */
let typingCleanupInterval: ReturnType<typeof setInterval> | null = null;

export const useChannelsStore = create<ChannelsState>((set, get) => ({
  channels: [],
  activeChannelId: null,
  activeChannelE2e: false,
  activeChannelKeyState: 'ready' as ChannelKeyState,
  activeChannelKeyError: null,
  joinedChannelIds: new Set<string>(),
  messages: [],
  wsConnected: false,
  loading: false,
  error: null,
  composerError: null,
  wsAuthRefreshing: false,
  historyLoading: false,
  historyError: null,
  loadingOlder: false,
  hasMoreMessages: true,
  olderError: null,
  editError: null,
  ws: null,
  pendingSends: new Map<string, PendingSend>(),
  typingUsers: [],
  unreadCounts: {},
  lastReadMessageIds: {},
  replyToMessage: null,
  typingSentAtByChannel: {},

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
      const { ws } = get();
      if (ws?.connected) {
        for (const channelId of joined) ws.subscribe(channelId);
      }
    } catch (err) {
      set({ channels: [], error: err instanceof Error ? err.message : String(err) });
    } finally {
      set({ loading: false });
    }
  },

  selectChannel: async (pseudonymId: string, channelId: string) => {
    const requestedChannelId = channelId;
    const { ws } = get();
    // Note: we intentionally do NOT unsubscribe from the previous channel.
    // Staying subscribed to all joined channels lets us receive messages
    // for non-active channels and increment unread counts accurately.

    set({ activeChannelId: channelId, activeChannelE2e: false, activeChannelKeyState: 'ready', activeChannelKeyError: null, messages: [], loadingOlder: false, hasMoreMessages: true, olderError: null, editError: null, historyLoading: true, historyError: null, composerError: null, typingUsers: [], replyToMessage: null });

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

    // Determine whether this channel is end-to-end encrypted and, if so, warm
    // the channel key (publish our device key + resolve/provision the CEK)
    // before any send/receive. Best-effort: a failure here just leaves the
    // channel treated as non-E2E for this session.
    try {
      const e2e = await api.getChannelE2e(pseudonymId, channelId);
      markChannelE2e(channelId, e2e);
      if (get().activeChannelId === channelId) set({ activeChannelE2e: e2e });
      if (e2e) {
        await ensureChannelReady(channelId);
        if (get().activeChannelId === channelId) {
          set({
            activeChannelKeyState: getChannelKeyState(channelId),
            activeChannelKeyError: getChannelKeyError(channelId),
          });
        }
      }
    } catch (err) {
      // Unknown, NOT plaintext. This used to record `false`, which is the
      // same value a genuinely unencrypted channel has, so the send path
      // took its plaintext branch and put unencrypted content on the wire
      // for the rest of the session — in a channel the user had turned
      // encryption on for. A failed check is not evidence of anything.
      console.warn('[channels] could not determine channel encryption:', err);
      markChannelE2eUnknown(channelId);
      if (get().activeChannelId === channelId) {
        set({
          composerError:
            'Could not confirm this channel is encrypted. Reload before sending.',
        });
      }
    }

    // Fetch history BEFORE subscribing to WS to avoid a race where
    // messages arriving between subscribe() and getMessages() completion
    // are silently dropped when the history response replaces the array.
    try {
      const messages = await api.getMessages(pseudonymId, requestedChannelId, undefined, PAGE_SIZE);
      if (get().activeChannelId !== requestedChannelId) return;
      const reversed = messages.reverse();
      // Decrypt E2E history bodies for display. Driven per-message by the body
      // marker, so it works even if E2E was later disabled (old ciphertext still
      // decrypts) or enabled (old plaintext passes through).
      if (isChannelE2e(requestedChannelId) || reversed.some((m) => isE2eBody(m.content))) {
        await Promise.all(
          reversed.map(async (m) => {
            if (m.content && !m.deleted_at) {
              m.content = await decryptForDisplay(requestedChannelId, m.content);
            }
          }),
        );
        if (get().activeChannelId !== requestedChannelId) return;
      }
      set((state) => {
        const historyById = new Set(reversed.map((m) => m.message_id).filter(Boolean));
        const liveDuringHydration = state.messages.filter((m) => !m.pending && (!m.message_id || !historyById.has(m.message_id)));
        return {
          messages: [...reversed, ...liveDuringHydration],
          hasMoreMessages: messages.length >= PAGE_SIZE,
          historyLoading: false,
          historyError: null,
        };
      });
      const newestMessageId = reversed[reversed.length - 1]?.message_id;
      get().markChannelRead(requestedChannelId, newestMessageId);
      // Track the newest message ID for resume
      if (reversed.length > 0 && ws) {
        ws.trackLastMessageId(requestedChannelId, reversed[reversed.length - 1].message_id);
      }
    } catch (err) {
      if (get().activeChannelId !== requestedChannelId) return;
      // Keep the channel selected but surface the history error so the
      // message pane can show a retry affordance instead of staying blank.
      set({
        historyLoading: false,
        historyError: err instanceof Error ? err.message : 'Failed to load channel history',
      });
    }

    // Subscribe to real-time updates AFTER history is loaded so no
    // messages are lost in the gap.
    if (get().activeChannelId !== requestedChannelId) return;
    if (ws) {
      ws.subscribe(requestedChannelId);
    }
  },

  retryChannelKey: async (pseudonymId: string) => {
    const channelId = get().activeChannelId;
    if (!channelId || !isChannelE2e(channelId)) return;
    set({ activeChannelKeyState: 'ready', activeChannelKeyError: null });
    await ensureChannelReady(channelId);
    if (get().activeChannelId !== channelId) return;
    const state = getChannelKeyState(channelId);
    set({
      activeChannelKeyState: state,
      activeChannelKeyError: getChannelKeyError(channelId),
    });
    // Landing the key means the history on screen is a page of placeholders
    // that can now be read. Reload so the messages actually appear — leaving
    // them is the same silence this whole state exists to end.
    if (state === 'ready') await get().selectChannel(pseudonymId, channelId);
  },

  enableChannelE2e: async (pseudonymId: string) => {
    const channelId = get().activeChannelId;
    if (!channelId) return;
    // Mark the channel E2E on the server (moderator-gated server-side).
    await api.setChannelE2e(pseudonymId, channelId, true);
    markChannelE2e(channelId, true);
    if (get().activeChannelId === channelId) set({ activeChannelE2e: true });
    // Provision/resolve the channel key so the next message encrypts.
    await ensureChannelReady(channelId);
    if (get().activeChannelId === channelId) {
      set({
        activeChannelKeyState: getChannelKeyState(channelId),
        activeChannelKeyError: getChannelKeyError(channelId),
      });
    }
  },

  connectWs: (pseudonymId: string, baseUrl?: string, sessionToken?: string | null) => {
    // Bind this identity for E2E channel encryption/decryption.
    setE2eIdentity(pseudonymId);
    const existing = get().ws;
    if (existing) existing.disconnect();

    // Clear typing cleanup interval
    if (typingCleanupInterval) {
      clearInterval(typingCleanupInterval);
      typingCleanupInterval = null;
    }

    const ws = new AnnexWebSocket(pseudonymId, baseUrl, sessionToken ?? null);
    const subscribeJoinedChannels = () => {
      const { joinedChannelIds } = get();
      for (const channelId of joinedChannelIds) ws.subscribe(channelId);
    };

    ws.onStatus((connected) => {
      set({ wsConnected: connected, wsAuthRefreshing: connected ? false : get().wsAuthRefreshing });
      if (connected) subscribeJoinedChannels();
    });

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
        set((state) => {
          const typingSentAtByChannel = { ...state.typingSentAtByChannel };
          delete typingSentAtByChannel[frame.channelId!];
          return {
            channels: state.channels.filter((c) => c.channel_id !== frame.channelId),
            typingSentAtByChannel,
          };
        });
        // If the deleted channel was active, clear the view
        if (get().activeChannelId === frame.channelId) {
          set({ activeChannelId: null, messages: [], typingUsers: [] });
        }
        return;
      }

      // E2E was toggled on a channel: update our cached flag immediately so the
      // send path encrypts (or stops encrypting) without waiting for a reload.
      if (frame.type === 'channel_e2e_changed' && frame.channelId) {
        const enabled = !!frame.e2eEnabled;
        markChannelE2e(frame.channelId, enabled);
        if (get().activeChannelId === frame.channelId) {
          set({ activeChannelE2e: enabled });
          // Warm the channel key so the very next message can be encrypted.
          if (enabled) void ensureChannelReady(frame.channelId);
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
        // A completed resume needs nothing: the missed messages arrived as
        // ordinary `message` frames before this ack.
        //
        // `cursorLost` is the other outcome, and it does need something. It
        // means the server could not work out what this client missed —
        // the message id being resumed from no longer names a live message
        // in the channel, which is what retention does on a schedule. The
        // replay was empty and the count is meaningless, so the timeline on
        // screen has a hole of unknown size. Refetch it. Doing nothing here
        // is what made a purged cursor indistinguishable from being up to
        // date. Only for the channel in view — `selectChannel` moves the
        // user, and any other channel reloads from scratch when opened.
        if (frame.cursorLost && frame.channelId && frame.channelId === get().activeChannelId) {
          void get().selectChannel(pseudonymId, frame.channelId);
        }
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
            // Never surface E2E ciphertext in an OS notification (per-message).
            const body = isE2eBody(frame.content ?? '')
              ? 'New encrypted message'
              : (frame.content ?? '').slice(0, 100);
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
        const bodyIsE2e = isE2eBody(msg.content);
        set((state) => {
          // If this message confirms an optimistic send, replace it
          if (frame.clientRequestId) {
            const optimistic = state.messages.find((m) => m.clientRequestId === frame.clientRequestId);
            if (optimistic) {
              // When the echoed wire `content` is E2E ciphertext, keep the
              // plaintext we already hold from the composer rather than
              // decrypting our own echo.
              //
              // `clientRequestId` is carried over deliberately. MessageView
              // keys rows `msg.clientRequestId ?? msg.message_id`; dropping it
              // here flipped the key on confirmation, so React unmounted the
              // row and mounted a new one. A user editing a message they had
              // just posted — the only time editing is offered, and exactly
              // when the confirmation is still in flight — lost the textarea
              // and had their half-typed edit reset to the original, silently.
              // The field is not a pending marker (`pending` and `failed`
              // are), so keeping it changes nothing else.
              const confirmed = bodyIsE2e
                ? { ...msg, content: optimistic.content, clientRequestId: frame.clientRequestId }
                : { ...msg, clientRequestId: frame.clientRequestId };
              const updated = state.messages.map((m) =>
                m.clientRequestId === frame.clientRequestId ? confirmed : m,
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
        // Inbound E2E message from someone else: decrypt asynchronously and
        // patch the body in by message_id (own echoes already hold plaintext).
        if (bodyIsE2e && msg.message_id && !frame.clientRequestId && msg.content) {
          const cipherChannelId = frame.channelId!;
          const targetId = msg.message_id;
          const cipher = msg.content;
          void decryptForDisplay(cipherChannelId, cipher).then((plain) => {
            set((state) => ({
              messages: state.messages.map((m) =>
                m.message_id === targetId ? { ...m, content: plain } : m,
              ),
            }));
          });
        }
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
        const editedContent = frame.content ?? '';
        set((state) => ({
          messages: state.messages.map((m) =>
            m.message_id === frame.messageId
              ? { ...m, content: editedContent || m.content, edited_at: frame.editedAt ?? null }
              : m,
          ),
        }));
        // Decrypt the edited body when it's E2E ciphertext (per-message).
        if (frame.channelId && frame.messageId && isE2eBody(editedContent)) {
          const ch = frame.channelId;
          const mid = frame.messageId;
          void decryptForDisplay(ch, editedContent).then((plain) => {
            set((state) => ({
              messages: state.messages.map((m) =>
                m.message_id === mid ? { ...m, content: plain } : m,
              ),
            }));
          });
        }
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
    if (ws.connected) subscribeJoinedChannels();

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

    const channelId = activeChannelId;

    // The optimistic message always shows the PLAINTEXT the user typed, even
    // on E2E channels where the wire body is ciphertext.
    const addOptimistic = (clientRequestId: string) => {
      const pending: PendingSend = { clientRequestId, content, sentAt: Date.now() };
      const optimisticMsg: Message = {
        message_id: '',
        channel_id: channelId,
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
    };
    const failOptimistic = (clientRequestId: string, err: unknown) => {
      get().resolvePendingSend(clientRequestId);
      set((s) => ({
        messages: s.messages.map((m) =>
          m.clientRequestId === clientRequestId ? { ...m, pending: false, failed: true } : m,
        ),
        composerError: err instanceof Error ? err.message : 'Failed to send message',
      }));
    };

    // Refuse to send while the encryption state is unresolved.
    //
    // The rule below — never put plaintext in an E2E channel — can only hold
    // if "not E2E" means the server said so. When the check failed, sending
    // anything is a guess, and the wrong guess is unrecoverable.
    if (isChannelE2eUnknown(channelId)) {
      set({
        composerError:
          'Could not confirm this channel is encrypted. Reload before sending.',
      });
      return null;
    }

    // Plaintext channels: unchanged synchronous path (ws.send mints the id).
    if (!isChannelE2e(channelId)) {
      try {
        const clientRequestId = ws.send(channelId, content, effectiveReplyTo);
        addOptimistic(clientRequestId);
        return clientRequestId;
      } catch (err) {
        console.error('[channels] sendMessage threw:', err);
        set({ composerError: err instanceof Error ? err.message : 'Failed to send message' });
        return null;
      }
    }

    // E2E channels: show plaintext optimistically, encrypt, then put ciphertext
    // on the wire under the same request id. NEVER send plaintext to an E2E
    // channel — a failure fails the message instead.
    const clientRequestId = crypto.randomUUID();
    addOptimistic(clientRequestId);
    void encryptForWire(channelId, content)
      .then((cipher) => ws.sendWithRequestId(channelId, cipher, effectiveReplyTo, clientRequestId))
      .catch((err) => {
        console.error('[channels] E2E send failed:', err);
        failOptimistic(clientRequestId, err);
      });
    return clientRequestId;
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
    const channelId = activeChannelId;

    // Keep the old body so the optimistic update can be undone. An edit that
    // never left the device used to stay on screen looking saved: the socket
    // was down, `ws.editMessage` threw, the catch logged to a console nobody
    // has open, and the user watched their correction apply. It survived
    // until the next reload, at which point the original text came back and
    // the edit was simply gone.
    const previous = get().messages.find((m) => m.message_id === messageId);
    const previousContent = previous?.content;
    const previousEditedAt = previous?.edited_at;

    // Optimistically show the new plaintext locally.
    set((s) => ({
      messages: s.messages.map((m) =>
        m.message_id === messageId ? { ...m, content, edited_at: new Date().toISOString() } : m,
      ),
    }));

    const revert = (err: unknown) => {
      console.error('[channels] editMessage failed:', err);
      if (previousContent === undefined) return;
      set((s) => ({
        messages: s.messages.map((m) =>
          m.message_id === messageId
            ? { ...m, content: previousContent, edited_at: previousEditedAt }
            : m,
        ),
        // Surfaced in the composer area rather than swallowed: the whole
        // point is that the user finds out now, while they still have the
        // text they meant to send.
        editError: 'Edit not saved — you appear to be offline. Try again.',
      }));
    };

    try {
      if (!isChannelE2e(channelId)) {
        ws.editMessage(channelId, messageId, content);
      } else {
        // Encrypt the edited body before it leaves the device.
        void encryptForWire(channelId, content)
          .then((cipher) => ws.editMessage(channelId, messageId, cipher))
          .catch(revert);
      }
    } catch (err) {
      revert(err);
    }
  },

  clearEditError: () => set({ editError: null }),

  deleteMessage: (messageId: string) => {
    const { ws, activeChannelId } = get();
    if (!ws || !activeChannelId) return;
    try {
      ws.deleteMessage(activeChannelId, messageId);
    } catch (err) {
      console.error('[channels] deleteMessage threw:', err);
    }
  },

  retryOlderMessages: async (pseudonymId: string) => {
    set({ olderError: null });
    await get().loadOlderMessages(pseudonymId);
  },

  loadOlderMessages: async (pseudonymId: string) => {
    const { activeChannelId, messages, loadingOlder, hasMoreMessages, olderError } = get();
    if (!activeChannelId || messages.length === 0 || loadingOlder || !hasMoreMessages) return;
    // A previous page failed. Do not retry on every scroll event — that is
    // the loop the old `hasMoreMessages: false` existed to prevent. Retry is
    // available, but the user has to ask for it.
    if (olderError) return;

    set({ loadingOlder: true });
    try {
      // Find the oldest non-pending message for cursor
      const oldest = messages.find((m) => !m.pending && !m.failed);
      if (!oldest) { set({ loadingOlder: false }); return; }
      const older = await api.getMessages(pseudonymId, activeChannelId, oldest.message_id, PAGE_SIZE);
      // The user may have switched channels while the request was in
      // flight — merging would splice another channel's history into the
      // current view, so drop the stale response instead.
      if (get().activeChannelId !== activeChannelId) return;
      const olderReversed = older.reverse();
      // Decrypt older E2E history bodies for display (per-message marker).
      if (isChannelE2e(activeChannelId) || olderReversed.some((m) => isE2eBody(m.content))) {
        await Promise.all(
          olderReversed.map(async (m) => {
            if (m.content && !m.deleted_at) {
              m.content = await decryptForDisplay(activeChannelId, m.content);
            }
          }),
        );
        if (get().activeChannelId !== activeChannelId) return;
      }
      set((state) => ({
        messages: [...olderReversed, ...state.messages],
        hasMoreMessages: older.length >= PAGE_SIZE,
        olderError: null,
      }));
    } catch (err) {
      console.warn('[channels] loadOlderMessages failed:', err);
      // Record the failure instead of declaring the history finished.
      //
      // Setting `hasMoreMessages: false` here did stop the scroll handler
      // retrying in a loop, and it also told the user they had reached the
      // beginning of the channel. One dropped request — a flaky network, a
      // token refresh, a server restart — permanently ended scrollback for
      // that channel, and looked identical to a short history. There was no
      // way to tell the difference and no way to retry.
      //
      // `olderError` suppresses the automatic retry the same way, because
      // the scroll handler checks it; the difference is that the UI can now
      // say what happened and offer the retry explicitly.
      if (get().activeChannelId === activeChannelId) {
        set({ olderError: 'Could not load earlier messages.' });
      }
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
    const { ws } = get();
    if (ws?.connected) ws.subscribe(channelId);
  },

  leaveChannel: async (pseudonymId, channelId) => {
    await api.leaveChannel(pseudonymId, channelId);

    set((s) => {
      const joined = new Set(s.joinedChannelIds);
      joined.delete(channelId);
      const typingSentAtByChannel = { ...s.typingSentAtByChannel };
      delete typingSentAtByChannel[channelId];
      return { joinedChannelIds: joined, typingSentAtByChannel };
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

    const { activeChannelId, ws } = get();
    if (ws) ws.unsubscribe(channelId);
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
    // Drop E2E identity + channel flags so nothing bleeds across sessions.
    setE2eIdentity(null);
    resetE2eChannels();
    clearE2eManagers();
  },

  updateWsSessionToken: (token: string | null) => {
    const { ws } = get();
    if (!ws) return;
    if (ws.connected) {
      set({ wsAuthRefreshing: true });
      ws.reconnectForAuthRefresh(token);
      return;
    }
    ws.setSessionToken(token);
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
      wsAuthRefreshing: false,
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
      typingSentAtByChannel: {},
    });
  },

  sendTyping: () => {
    const { ws, activeChannelId, typingSentAtByChannel } = get();
    if (!ws || !activeChannelId) return;
    const now = Date.now();
    const lastTypingSentAt = typingSentAtByChannel[activeChannelId] ?? 0;
    if (now - lastTypingSentAt < TYPING_DEBOUNCE_MS) return;
    set((state) => ({
      typingSentAtByChannel: {
        ...state.typingSentAtByChannel,
        [activeChannelId]: now,
      },
    }));
    ws.sendTyping(activeChannelId);
  },

  markChannelRead: (channelId: string, lastMessageId?: string) => {
    const { messages } = get();
    const resolvedLastMessageId = lastMessageId
      ?? [...messages].reverse().find((msg) => msg.channel_id === channelId)?.message_id;
    set((state) => ({
      unreadCounts: { ...state.unreadCounts, [channelId]: 0 },
      lastReadMessageIds: resolvedLastMessageId
        ? { ...state.lastReadMessageIds, [channelId]: resolvedLastMessageId }
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
      wsAuthRefreshing: false,
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
