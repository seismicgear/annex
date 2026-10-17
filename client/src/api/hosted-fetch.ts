import { HostTunnel, hostedRoute } from '../lib/host-tunnel';

/** Encrypt every hosted request before it reaches the public router.
 * Direct server requests retain fetch semantics. A disconnected write is never
 * automatically replayed: its outcome may already have reached the host.
 */
export function transportFetch(url: string, init?: RequestInit): Promise<Response> {
  const route = hostedRoute(url);
  if (!route) return fetch(url, init);
  return new Promise<Response>((resolve, reject) => {
    let socket: HostTunnel;
    let complete = false, started = false, status = 0, responseSize = 0;
    let inFlight = 0, window = 1;
    let headers: HeadersInit = {}, acknowledge: (() => void) | undefined;
    let rejectAcknowledgement: ((reason: Error) => void) | undefined;
    const response: Uint8Array<ArrayBuffer>[] = [];
    const request = new Request(url, init);
    const reader = request.body?.getReader();
    const cleanup = () => {
      clearTimeout(timeout); init?.signal?.removeEventListener('abort', abort);
      void reader?.cancel().catch(() => {});
      if (socket && socket.readyState < WebSocket.CLOSING) socket.close();
    };
    const fail = (reason: unknown) => {
      if (complete) return; complete = true;
      rejectAcknowledgement?.(new Error('Upload ended')); cleanup(); reject(reason);
    };
    const abort = () => fail(init?.signal?.reason ?? new DOMException('Upload cancelled', 'AbortError'));
    const transferring = request.method === 'POST' && /\/(upload|encrypted-upload|image)$/.test(route.path.split('?')[0])
      || request.method === 'GET' && route.path.startsWith('/uploads/');
    const timeout = setTimeout(() => fail(new Error('The server request timed out. Check the server before retrying.')), transferring ? 240_000 : 120_000);
    if (init?.signal?.aborted) { abort(); return; }
    init?.signal?.addEventListener('abort', abort, { once: true });
    try { socket = new HostTunnel(route); }
    catch (error) { fail(error); return; }
    const upload = async () => {
      try {
        while (reader && !complete) {
          const next = await reader.read();
          if (next.done) break;
          for (let offset = 0; offset < next.value.length && !complete; offset += 64 * 1024) {
            const chunk = next.value.subarray(offset, offset + 64 * 1024);
            if (inFlight >= window) {
              await new Promise<void>((resolve, reject) => { acknowledge = resolve; rejectAcknowledgement = reject; });
              acknowledge = undefined; rejectAcknowledgement = undefined;
            }
            if (complete) return;
            // This bounded chunk avoids both argument-stack and unbounded
            // socket-queue growth for the server's largest permitted file.
            let binary = ''; for (const byte of chunk) binary += String.fromCharCode(byte);
            inFlight++; socket.send(JSON.stringify({ type: 'http_data', data: btoa(binary) }));
          }
        }
        if (!complete) socket.send(JSON.stringify({ type: 'http_end' }));
      } catch (error) { fail(error); }
    };
    socket.onopen = () => {
      if (complete) { socket.close(); return; }
      socket.send(JSON.stringify({ type: 'http_open', method: request.method, path: route.path, headers: Object.fromEntries(request.headers) }));
    };
    socket.onmessage = event => {
      if (complete) return;
      try {
        const frame = JSON.parse(String(event.data));
        if (frame.type === 'request_ready' && !started) {
          window = Number.isInteger(frame.window) && frame.window >= 1 && frame.window <= 32 ? frame.window : 1;
          started = true; void upload();
        }
        else if (frame.type === 'request_ack' && inFlight > 0) { inFlight--; acknowledge?.(); }
        else if (frame.type === 'response' && Number.isInteger(frame.status) && frame.status >= 200 && frame.status <= 599) { status = frame.status; headers = frame.headers; }
        else if (frame.type === 'data' && typeof frame.data === 'string' && frame.data.length <= 100_000) {
          const binary = atob(frame.data); responseSize += binary.length;
          if (responseSize > 64 * 1024 * 1024) throw new Error('Hosted response exceeded its limit');
          response.push(Uint8Array.from(binary, char => char.charCodeAt(0)));
          socket.send(JSON.stringify({ type: 'response_ack' }));
        } else if (frame.type === 'end' && status) {
          if (status >= 300 && status < 400 && status !== 304 && init?.redirect !== 'manual') throw new Error('A hosted request redirected outside its encrypted connection');
          const value = new Response(request.method === 'HEAD' || [204, 205, 304].includes(status) ? null : new Blob(response), { status, headers });
          complete = true; rejectAcknowledgement?.(new Error('Upload completed')); cleanup(); resolve(value);
        } else if (frame.type === 'error') throw new Error('The host could not complete the request. Check the server before retrying.');
      } catch (error) { fail(error); }
    };
    socket.onerror = () => fail(new Error('Could not connect to the public host.'));
    socket.onclose = () => fail(new Error('The server connection closed. Check the server before retrying.'));
  });
}
