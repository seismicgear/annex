/**
 * WebSocket client for real-time messaging.
 *
 * Connects to the server's /ws endpoint with pseudonym authentication.
 * Handles reconnection with exponential backoff.
 */

import type { WsSendFrame, WsReceiveFrame } from '@/types';

export type WsMessageHandler = (frame: WsReceiveFrame) => void;
export type WsStatusHandler = (connected: boolean) => void;

const MAX_RECONNECT_DELAY_MS = 30_000;
const INITIAL_RECONNECT_DELAY_MS = 1_000;

export class AnnexWebSocket {
  private ws: WebSocket | null = null;
  private pseudonymId: string;
  private baseUrl: string;
  private messageHandlers: Set<WsMessageHandler> = new Set();
  private statusHandlers: Set<WsStatusHandler> = new Set();
  private reconnectDelay = INITIAL_RECONNECT_DELAY_MS;
  private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
  private intentionalClose = false;

  /**
   * @param pseudonymId — identity pseudonym for auth
   * @param baseUrl — server base URL (e.g. "https://annex.example.com"). Empty for current origin.
   */
  constructor(pseudonymId: string, baseUrl = '') {
    this.pseudonymId = pseudonymId;
    this.baseUrl = baseUrl.replace(/\/+$/, '');
  }

  /** Connect to the WebSocket server. */
  connect(): void {
    this.intentionalClose = false;

    let url: string;
    if (this.baseUrl) {
      // Cross-server: convert http(s) URL to ws(s) URL
      const wsBase = this.baseUrl.replace(/^http/, 'ws');
      url = `${wsBase}/ws?pseudonym=${encodeURIComponent(this.pseudonymId)}`;
    } else {
      const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
      const host = window.location.host;
      url = `${protocol}//${host}/ws?pseudonym=${encodeURIComponent(this.pseudonymId)}`;
    }

    this.ws = new WebSocket(url);

    this.ws.onopen = () => {
      this.reconnectDelay = INITIAL_RECONNECT_DELAY_MS;
      this.notifyStatus(true);
    };

    this.ws.onclose = () => {
      this.notifyStatus(false);
      if (!this.intentionalClose) {
        this.scheduleReconnect();
      }
    };

    this.ws.onerror = () => {
      // onclose will fire after onerror
    };

    this.ws.onmessage = (event) => {
      try {
        const frame: WsReceiveFrame = JSON.parse(event.data as string);
        this.messageHandlers.forEach((h) => h(frame));
      } catch {
        // Malformed frame — drop silently. This can happen during protocol
        // version mismatches or if the server sends a non-JSON control frame.
      }
    };
  }

  /** Subscribe to real-time messages for a channel. */
  subscribe(channelId: string): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    this.ws.send(JSON.stringify({ type: 'subscribe', channelId }));
  }

  /** Unsubscribe from a channel's real-time messages. */
  unsubscribe(channelId: string): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) return;
    this.ws.send(JSON.stringify({ type: 'unsubscribe', channelId }));
  }

  /** Send a message to a channel. */
  send(channelId: string, content: string, replyTo: string | null = null): void {
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      throw new Error('WebSocket is not connected');
    }
    const frame: WsSendFrame = {
      type: 'message',
      channelId,
      content,
      replyTo,
    };
    this.ws.send(JSON.stringify(frame));
  }

  /** Register a handler for incoming messages. */
  onMessage(handler: WsMessageHandler): () => void {
    this.messageHandlers.add(handler);
    return () => this.messageHandlers.delete(handler);
  }

  /** Register a handler for connection status changes. */
  onStatus(handler: WsStatusHandler): () => void {
    this.statusHandlers.add(handler);
    return () => this.statusHandlers.delete(handler);
  }

  /** Close the connection intentionally. */
  disconnect(): void {
    this.intentionalClose = true;
    if (this.reconnectTimer) {
      clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (this.ws) {
      this.ws.close();
      this.ws = null;
    }
  }

  /** Whether the socket is currently connected. */
  get connected(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  private notifyStatus(connected: boolean): void {
    this.statusHandlers.forEach((h) => h(connected));
  }

  private scheduleReconnect(): void {
    this.reconnectTimer = setTimeout(() => {
      this.connect();
    }, this.reconnectDelay);
    this.reconnectDelay = Math.min(this.reconnectDelay * 2, MAX_RECONNECT_DELAY_MS);
  }
}
