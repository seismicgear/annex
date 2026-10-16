import { clientHello, fromBase64, toBase64, type TunnelCipher } from './host-tunnel-crypto';

export interface HostedRoute { socket: string; host: string; path: string }
export function hostedRoute(value: string): HostedRoute | null {
  let url: URL; try { url = new URL(value); } catch { return null; }
  if (!['https:', 'wss:'].includes(url.protocol) || url.port || url.username || url.password
    || !['monolithannex.com', 'www.monolithannex.com'].includes(url.hostname)) return null;
  const match = /^(\/host\/([0-9a-f]{64}))(\/.*)$/.exec(url.pathname);
  if (!match) return null;
  return { socket: `wss://${url.host}${match[1]}/__tunnel`, host: match[2], path: match[3] + url.search };
}

export type AppSocket = Pick<WebSocket, 'readyState' | 'send' | 'close' | 'onopen' | 'onclose' | 'onerror' | 'onmessage'>;

/** WebSocket-shaped endpoint adapter. The public socket never receives a
 * plaintext application frame, URL query, token, request header or body.
 */
export class HostTunnel implements AppSocket {
  readyState: number = WebSocket.CONNECTING;
  onopen: WebSocket['onopen'] = null;
  onclose: WebSocket['onclose'] = null;
  onerror: WebSocket['onerror'] = null;
  onmessage: WebSocket['onmessage'] = null;
  private socket: WebSocket;
  private hello = clientHello();
  private cipher?: TunnelCipher;
  private timeout: ReturnType<typeof setTimeout>;
  private closeCode = 0;
  constructor(route: HostedRoute) {
    this.socket = new WebSocket(route.socket);
    this.timeout = setTimeout(() => this.fail(), 20_000);
    this.socket.onopen = () => this.socket.send(JSON.stringify({ type: 'tunnel_open', hello: this.hello.hello }));
    this.socket.onmessage = event => {
      if (this.readyState >= WebSocket.CLOSING) return;
      try {
        if (typeof event.data !== 'string' || event.data.length > 128 * 1024) throw new Error('Invalid tunnel packet');
        const packet = JSON.parse(event.data);
        if (!this.cipher && packet.type === 'tunnel_hello') {
          this.cipher = this.hello.finish(route.host, packet.hello);
          clearTimeout(this.timeout); this.readyState = WebSocket.OPEN;
          this.onopen?.call(this as unknown as WebSocket, new Event('open'));
        } else if (this.cipher && packet.type === 'tunnel_data') {
          const value = this.cipher.open(packet);
          if (value !== undefined) this.onmessage?.call(this as unknown as WebSocket, new MessageEvent('message', { data: JSON.stringify(value) }));
        } else throw new Error('Invalid tunnel state');
      } catch { this.fail(); }
    };
    this.socket.onerror = () => this.fail();
    this.socket.onclose = event => {
      clearTimeout(this.timeout); this.hello.destroy(); this.cipher?.destroy();
      this.readyState = WebSocket.CLOSED;
      this.onclose?.call(this as unknown as WebSocket, new CloseEvent('close', {
        code: this.closeCode || event.code, reason: this.closeCode ? 'Encrypted connection interrupted' : event.reason,
        wasClean: !this.closeCode && event.wasClean,
      }));
    };
  }
  send(data: string | ArrayBufferLike | Blob | ArrayBufferView): void {
    if (this.readyState !== WebSocket.OPEN || !this.cipher) throw new DOMException('Tunnel is not open', 'InvalidStateError');
    if (typeof data !== 'string' || this.socket.bufferedAmount > 8 * 1024 * 1024) { this.fail(); throw new Error('Tunnel capacity exceeded'); }
    try {
      for (const packet of this.cipher.seal(JSON.parse(data))) this.socket.send(JSON.stringify({ type: 'tunnel_data', ...packet }));
    } catch (error) { this.fail(); throw error; }
  }
  close(code = 1000, reason = '') {
    if (this.readyState >= WebSocket.CLOSING) return;
    this.readyState = WebSocket.CLOSING; clearTimeout(this.timeout);
    this.hello.destroy(); this.cipher?.destroy();
    this.socket.close(code, reason);
  }
  private fail() {
    if (this.readyState >= WebSocket.CLOSING) return;
    this.closeCode = 1011;
    this.onerror?.call(this as unknown as WebSocket, new Event('error'));
    this.close();
  }
}

class HostedAppSocket implements AppSocket {
  readyState: number = WebSocket.CONNECTING;
  onopen: WebSocket['onopen'] = null;
  onclose: WebSocket['onclose'] = null;
  onerror: WebSocket['onerror'] = null;
  onmessage: WebSocket['onmessage'] = null;
  private tunnel: HostTunnel;
  private upstreamClose = false;
  constructor(route: HostedRoute) {
    this.tunnel = new HostTunnel(route);
    this.tunnel.onopen = () => this.tunnel.send(JSON.stringify({ type: 'ws_open', path: route.path, headers: {} }));
    this.tunnel.onerror = event => this.onerror?.call(this as unknown as WebSocket, event);
    this.tunnel.onclose = event => {
      this.readyState = WebSocket.CLOSED;
      if (!this.upstreamClose) this.onclose?.call(this as unknown as WebSocket, event);
    };
    this.tunnel.onmessage = event => {
      try {
        const frame = JSON.parse(String(event.data));
        if (frame.type === 'ws_ready' && this.readyState === WebSocket.CONNECTING) {
          this.readyState = WebSocket.OPEN; this.onopen?.call(this as unknown as WebSocket, new Event('open'));
        } else if (frame.type === 'ws_data' && this.readyState === WebSocket.OPEN) {
          const bytes = fromBase64(frame.data);
          const data = frame.binary ? new Blob([new Uint8Array(bytes)]) : new TextDecoder('utf-8', { fatal: true }).decode(bytes);
          this.onmessage?.call(this as unknown as WebSocket, new MessageEvent('message', { data }));
        } else if (frame.type === 'ws_close') {
          this.upstreamClose = true; this.readyState = WebSocket.CLOSED;
          this.onclose?.call(this as unknown as WebSocket, new CloseEvent('close', { code: frame.code || 1000, reason: frame.reason || '', wasClean: true }));
          this.tunnel.close();
        } else if (frame.type === 'error') throw new Error('Host rejected connection');
      } catch { this.tunnel.close(); }
    };
  }
  send(data: string | ArrayBufferLike | Blob | ArrayBufferView) {
    if (this.readyState !== WebSocket.OPEN) throw new DOMException('Socket is not open', 'InvalidStateError');
    if (typeof data !== 'string') throw new TypeError('Annex application frames must be text');
    this.tunnel.send(JSON.stringify({ type: 'ws_data', binary: false, data: toBase64(new TextEncoder().encode(data)) }));
  }
  close(code = 1000, reason = '') {
    if (this.readyState >= WebSocket.CLOSING) return;
    this.readyState = WebSocket.CLOSING;
    if (this.tunnel.readyState === WebSocket.OPEN) this.tunnel.send(JSON.stringify({ type: 'ws_close' }));
    this.tunnel.close(code, reason);
  }
}

export function transportSocket(url: string): AppSocket {
  const route = hostedRoute(url);
  return route ? new HostedAppSocket(route) : new WebSocket(url);
}
