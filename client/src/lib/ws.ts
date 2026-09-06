/**
 * WebSocket client for real-time messaging.
 *
 * Connects to the server's /ws endpoint with pseudonym authentication.
 * Handles reconnection with exponential backoff.
 */

import type { WsSendFrame, WsReceiveFrame } from '@/types';

export type WsMessageHandler = (frame: WsReceiveFrame) => void;
export type WsStatusHandler = (connected: boolean) => void;

const FRAME_TYPES_REQUIRING_CHANNEL_ID = new Set<WsReceiveFrame['type']>([
  'message',
  'message_edited',
  'message_deleted',
  'typing',
  'resumed',
  'transcription',
  'webrtc_answer',
  // The server offers when a peer joins or leaves a call and this peer's
  // track set changes. Without this in the allow-list the frame is dropped
  // before it reaches the session and the call never grows.
  'webrtc_offer',
  'webrtc_ice_candidate',
]);

const MAX_RECONNECT_DELAY_MS = 30_000;
const INITIAL_RECONNECT_DELAY_MS = 1_000;

export class AnnexWebSocket {
  private ws: WebSocket | null = null;
  private pseudonymId: string;
  private baseUrl: string;
  private sessionToken: string | null;
  private messageHandlers: Set<WsMessageHandler> = new Set();
  private statusHandlers: Set<WsStatusHandler> = new Set();
  private reconnectDelay = INITIAL_RECONNECT_DELAY_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private intentionalClose = false;
  /** Channels we should be subscribed to — re-sent on (re)connect. */
  private subscribedChannels: Set<string> = new Set();
  /** Last received message ID per channel — used for resume on reconnect. */
  private lastMessageIds: Map<string, string> = new Map();
  /** Whether this is a reconnection (not the first connect). */
  private hasConnectedBefore = false;

  /**
   * @param pseudonymId — identity pseudonym for auth
   * @param baseUrl — server base URL (e.g. "https://annex.example.com"). Empty for current origin.
   * @param sessionToken — HMAC-signed token. If provided, connects with ?token= instead of ?pseudonym=.
   */
  constructor(pseudonymId: string, baseUrl = '', sessionToken: string | null = null) {
    this.pseudonymId = pseudonymId;
    this.baseUrl = baseUrl.replace(/\/+$/, '');
    this.sessionToken = sessionToken;
  }

  /** Connect to the WebSocket server. */
  connect(): void {
    this.intentionalClose = false;

    // Build auth query parameter: prefer signed token over raw pseudonym.
    const authParam = this.sessionToken
      ? `token=${encodeURIComponent(this.sessionToken)}`
      : `pseudonym=${encodeURIComponent(this.pseudonymId)}`;

    let url: string;
    if (this.baseUrl) {
      const wsBase = this.baseUrl.replace(/^http/, 'ws');
      url = `${wsBase}/ws?${authParam}`;
    } else {
      const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
      const host = window.location.host;
      url = `${protocol}//${host}/ws?${authParam}`;
    }

    console.info('[ws] connecting to', url.replace(/token=[^&]+/, 'token=***'));

    // Tear down any still-live socket before replacing it, so an old
    // connection can't keep dispatching frames alongside the new one.
    if (this.ws && this.ws.readyState !== WebSocket.CLOSED) {
      try { this.ws.close(); } catch { /* already closing */ }
    }

    const socket = new WebSocket(url);
    this.ws = socket;

    // Every handler ignores events from sockets that have been replaced:
    // close events from a superseded socket arrive asynchronously and
    // must not trigger reconnects (which would create duplicate
    // connections) or status flaps for the current socket.
    socket.onopen = () => {
      if (this.ws !== socket) return;
      console.info('[ws] connected');
      this.reconnectDelay = INITIAL_RECONNECT_DELAY_MS;
      // Re-subscribe to all tracked channels on (re)connect
      for (const channelId of this.subscribedChannels) {
        socket.send(JSON.stringify({ type: 'subscribe', channelId }));
      }
      // Resume: replay missed messages on reconnect
      if (this.hasConnectedBefore) {
        for (const [channelId, lastMessageId] of this.lastMessageIds) {
          if (this.subscribedChannels.has(channelId)) {
            socket.send(JSON.stringify({ type: 'resume', channelId, lastMessageId }));
          }
        }
      }
      this.hasConnectedBefore = true;
      this.notifyStatus(true);
    };

    socket.onclose = (event) => {
      if (this.ws !== socket) return;
      console.warn('[ws] closed', { code: event.code, reason: event.reason, wasClean: event.wasClean });
      this.notifyStatus(false);
      if (!this.intentionalClose) {
        this.scheduleReconnect();
      }
    };

    socket.onerror = (event) => {
      console.error('[ws] error', event);
      // onclose will fire after onerror
    };

    socket.onmessage = (event) => {
      if (this.ws !== socket) return;
      const rawPayload = String(event.data ?? '');
      let frame: WsReceiveFrame;
      try {
        frame = JSON.parse(rawPayload) as WsReceiveFrame;
      } catch {
        this.emitInternalWarning('failed to parse websocket frame as JSON', rawPayload);
        return;
      }

      if (!this.isFrameDispatchable(frame)) {
        this.emitInternalWarning('received malformed websocket frame shape', rawPayload);
        return;
      }

      this.messageHandlers.forEach((h) => h(frame));
    };
  }

  /**
   * Swap to a fresh auth token immediately by reconnecting the socket.
   * Preserves tracked subscriptions and resume cursors.
   */
  reconnectForAuthRefresh(token: string | null): void {
    this.sessionToken = token;
    if (!this.ws || this.ws.readyState === WebSocket.CLOSED) return;
    this.intentionalClose = true;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    this.ws.close(4001, 'auth_refresh');
    this.ws = null;
    this.connect();
  }

  subscribe(channelId: string): void {
    this.subscribedChannels.add(channelId);
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    this.ws.send(JSON.stringify({ type: 'subscribe', channelId }));
  }

  unsubscribe(channelId: string): void {
    this.subscribedChannels.delete(channelId);
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    this.ws.send(JSON.stringify({ type: 'unsubscribe', channelId }));
  }

  send(channelId: string, content: string, replyTo: string | null = null): string {
    const clientRequestId = crypto.randomUUID();
    this.sendWithRequestId(channelId, content, replyTo, clientRequestId);
    return clientRequestId;
  }

  /**
   * Send a chat message frame with a caller-supplied `clientRequestId`. Used by
   * the E2E path, where the optimistic (plaintext) message is added to the UI
   * synchronously while the ciphertext is produced asynchronously and only then
   * put on the wire — both bound to the same request id.
   */
  sendWithRequestId(
    channelId: string,
    content: string,
    replyTo: string | null,
    clientRequestId: string,
  ): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      throw new Error('WebSocket is not connected');
    }
    const frame: WsSendFrame = { type: 'message', channelId, content, replyTo, clientRequestId };
    this.ws.send(JSON.stringify(frame));
  }

  editMessage(channelId: string, messageId: string, content: string, clientRequestId?: string): void { if (!this.ws || this.ws.readyState !== WebSocket.OPEN) throw new Error('WebSocket is not connected'); this.ws.send(JSON.stringify({ type: 'edit_message', channelId, messageId, content, clientRequestId } as WsSendFrame)); }
  deleteMessage(channelId: string, messageId: string, clientRequestId?: string): void { if (!this.ws || this.ws.readyState !== WebSocket.OPEN) throw new Error('WebSocket is not connected'); this.ws.send(JSON.stringify({ type: 'delete_message', channelId, messageId, clientRequestId } as WsSendFrame)); }
  trackLastMessageId(channelId: string, messageId: string): void { this.lastMessageIds.set(channelId, messageId); }
  sendTyping(channelId: string): void { if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return; this.ws.send(JSON.stringify({ type: 'typing', channelId })); }
  sendWebRtcOffer(channelId: string, sdp: string): void { if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return; this.ws.send(JSON.stringify({ type: 'webrtc_offer', channelId, sdp } as WsSendFrame)); }
  sendWebRtcAnswer(channelId: string, sdp: string): void { if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return; this.ws.send(JSON.stringify({ type: 'webrtc_answer', channelId, sdp } as WsSendFrame)); }
  sendIceCandidate(channelId: string, candidate: string, sdpMid: string | null = null, sdpMLineIndex: number | null = null): void { if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return; this.ws.send(JSON.stringify({ type: 'webrtc_ice_candidate', channelId, candidate, sdpMid, sdpMLineIndex } as WsSendFrame)); }

  onMessage(handler: WsMessageHandler): () => void { this.messageHandlers.add(handler); return () => this.messageHandlers.delete(handler); }
  onStatus(handler: WsStatusHandler): () => void { this.statusHandlers.add(handler); return () => this.statusHandlers.delete(handler); }

  disconnect(): void {
    this.intentionalClose = true;
    this.subscribedChannels.clear();
    this.lastMessageIds.clear();
    this.hasConnectedBefore = false;
    this.messageHandlers.clear();
    this.statusHandlers.clear();
    if (this.reconnectTimer) { clearTimeout(this.reconnectTimer); this.reconnectTimer = null; }
    if (this.ws) { this.ws.close(); this.ws = null; }
  }

  setSessionToken(token: string | null): void { this.sessionToken = token; }

  private emitInternalWarning(reason: string, rawPayload: unknown): void {
    const raw = typeof rawPayload === 'string' ? rawPayload : JSON.stringify(rawPayload);
    const preview = raw.length > 160 ? `${raw.slice(0, 160)}…` : raw;
    console.warn(`[ws] ${reason}`, { payloadPreview: preview });
    this.messageHandlers.forEach((handler) =>
      handler({
        type: 'internal_error',
        error: reason,
        message: `WebSocket warning: ${reason}`,
        rawPayloadPreview: preview,
      }),
    );
  }

  private isFrameDispatchable(frame: WsReceiveFrame): boolean {
    if (!frame || typeof frame !== 'object') return false;
    if (!frame.type || typeof frame.type !== 'string') return false;
    if (FRAME_TYPES_REQUIRING_CHANNEL_ID.has(frame.type) && !frame.channelId) return false;
    return true;
  }
  get connected(): boolean { return this.ws?.readyState === WebSocket.OPEN; }
  private notifyStatus(connected: boolean): void { this.statusHandlers.forEach((h) => h(connected)); }
  private scheduleReconnect(): void {
    // Replace (don't stack) any pending reconnect so overlapping close
    // events can't fan out into multiple parallel connections.
    if (this.reconnectTimer) clearTimeout(this.reconnectTimer);
    this.reconnectTimer = setTimeout(() => { this.reconnectTimer = null; this.connect(); }, this.reconnectDelay);
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, MAX_RECONNECT_DELAY_MS);
  }
}
