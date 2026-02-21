/**
 * Channel store — manages channel list, active channel, and messages.
 */

import { create } from 'zustand';
import type { Channel, Message, WsReceiveFrame } from '@/types';
import * as api from '@/lib/api';
import { AnnexWebSocket } from '@/lib/ws';

interface ChannelsState {
  /** All available channels. */
  channels: Channel[];
  /** Currently selected channel ID. */
  activeChannelId: string | null;
  /** Messages for the active channel (newest last). */
  messages: Message[];
  /** Whether the WebSocket is connected. */
  wsConnected: boolean;
  /** Loading state for channel list. */
  loading: boolean;
  /** The WebSocket instance (internal). */
  ws: AnnexWebSocket | null;

  /** Load channel list from server. */
  loadChannels: (pseudonymId: string) => Promise<void>;
  /** Select a channel and load its history. */
  selectChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Connect WebSocket for real-time messages. Optional baseUrl for cross-server. */
  connectWs: (pseudonymId: string, baseUrl?: string) => void;
  /** Send a message to the active channel. */
  sendMessage: (content: string, replyTo?: string | null) => void;
  /** Load older messages (pagination). */
  loadOlderMessages: (pseudonymId: string) => Promise<void>;
  /** Create a new channel. */
  createChannel: (pseudonymId: string, name: string, channelType: string, topic?: string, federated?: boolean) => Promise<Channel>;
  /** Join a channel. */
  joinChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Leave a channel. */
  leaveChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  /** Disconnect WebSocket. */
  disconnectWs: () => void;
}

export const useChannelsStore = create<ChannelsState>((set, get) => ({
  channels: [],
  activeChannelId: null,
  messages: [],
  wsConnected: false,
  loading: false,
  ws: null,

  loadChannels: async (pseudonymId: string) => {
    set({ loading: true });
    const channels = await api.listChannels(pseudonymId);
    set({ channels, loading: false });
  },

  selectChannel: async (pseudonymId: string, channelId: string) => {
    set({ activeChannelId: channelId, messages: [] });
    // Auto-join the channel (idempotent — no-op if already a member).
    // Must be a member before fetching messages or joining voice.
    try {
      await api.joinChannel(pseudonymId, channelId);
    } catch {
      // May fail for capability restrictions; still try to load messages
      // in case the user is already a member from a previous session.
    }
    const messages = await api.getMessages(pseudonymId, channelId, undefined, 50);
    set({ messages: messages.reverse() });
  },

  connectWs: (pseudonymId: string, baseUrl?: string) => {
    const existing = get().ws;
    if (existing) existing.disconnect();

    const ws = new AnnexWebSocket(pseudonymId, baseUrl);

    ws.onStatus((connected) => set({ wsConnected: connected }));

    ws.onMessage((frame: WsReceiveFrame) => {
      if (frame.type === 'message' && frame.channelId === get().activeChannelId) {
        const msg: Message = {
          message_id: frame.messageId ?? '',
          channel_id: frame.channelId,
          sender_pseudonym: frame.senderPseudonym ?? '',
          content: frame.content ?? '',
          reply_to_message_id: frame.replyToMessageId ?? null,
          created_at: frame.createdAt ?? new Date().toISOString(),
        };
        set((state) => ({ messages: [...state.messages, msg] }));
      }
    });

    ws.connect();
    set({ ws });
  },

  sendMessage: (content: string, replyTo: string | null = null) => {
    const { ws, activeChannelId } = get();
    if (!ws || !activeChannelId) return;
    ws.send(activeChannelId, content, replyTo);
  },

  loadOlderMessages: async (pseudonymId: string) => {
    const { activeChannelId, messages } = get();
    if (!activeChannelId || messages.length === 0) return;
    const oldest = messages[0];
    const older = await api.getMessages(pseudonymId, activeChannelId, oldest.message_id, 50);
    set({ messages: [...older.reverse(), ...messages] });
  },

  createChannel: async (pseudonymId, name, channelType, topic, federated) => {
    const channel = await api.createChannel(pseudonymId, name, channelType, topic, federated);
    set((state) => ({ channels: [...state.channels, channel] }));
    return channel;
  },

  joinChannel: async (pseudonymId, channelId) => {
    await api.joinChannel(pseudonymId, channelId);
  },

  leaveChannel: async (pseudonymId, channelId) => {
    await api.leaveChannel(pseudonymId, channelId);
    const { activeChannelId } = get();
    if (activeChannelId === channelId) {
      set({ activeChannelId: null, messages: [] });
    }
  },

  disconnectWs: () => {
    const { ws } = get();
    if (ws) {
      ws.disconnect();
      set({ ws: null, wsConnected: false });
    }
  },
}));
