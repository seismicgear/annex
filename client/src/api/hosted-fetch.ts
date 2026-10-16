/** Stream hosted multipart uploads without the gateway's HTTP body limit.
 * Ordinary requests retain fetch semantics. A disconnected write is never
 * automatically replayed: its outcome may already have reached the host.
 */
export function hostedUploadRoute(url: string, body: BodyInit | null | undefined): { socket: string; path: string } | null {
  if (!(body instanceof FormData || body instanceof Blob || (typeof body === 'string' && new Blob([body]).size > 4 * 1024 * 1024))) return null;
  let target: URL;
  try { target = new URL(url); } catch { return null; }
  if (target.protocol !== 'https:' || target.port || target.username || target.password
    || !['monolithannex.com', 'www.monolithannex.com'].includes(target.hostname)) return null;
  const route = /^(\/host\/[0-9a-f]{64})(\/api\/.*)$/.exec(target.pathname);
  if (!route) return null;
  return { socket: `wss://${target.host}${route[1]}/__request`, path: route[2] + target.search };
}

export function transportFetch(url: string, init?: RequestInit): Promise<Response> {
  const route = hostedUploadRoute(url, init?.body);
  if (!route) return fetch(url, init);
  return new Promise<Response>((resolve, reject) => {
    let socket: WebSocket;
    let complete = false, started = false, status = 0, responseSize = 0;
    let headers: HeadersInit = {}, acknowledge: (() => void) | undefined;
    let rejectAcknowledgement: ((reason: Error) => void) | undefined;
    const response: Uint8Array<ArrayBuffer>[] = [];
    const request = new Request(url, init);
    const reader = request.body?.getReader();
    const cleanup = () => {
      clearTimeout(timeout); init?.signal?.removeEventListener('abort', abort);
      void reader?.cancel().catch(() => {});
      if (socket?.readyState === WebSocket.OPEN) socket.close();
    };
    const fail = (reason: unknown) => {
      if (complete) return; complete = true;
      rejectAcknowledgement?.(new Error('Upload ended')); cleanup(); reject(reason);
    };
    const abort = () => fail(init?.signal?.reason ?? new DOMException('Upload cancelled', 'AbortError'));
    const timeout = setTimeout(() => fail(new Error('Upload timed out. Check the server before retrying.')), 120_000);
    if (init?.signal?.aborted) { abort(); return; }
    init?.signal?.addEventListener('abort', abort, { once: true });
    try { socket = new WebSocket(route.socket); }
    catch (error) { fail(error); return; }
    const upload = async () => {
      try {
        while (reader && !complete) {
          const next = await reader.read();
          if (next.done) break;
          for (let offset = 0; offset < next.value.length && !complete; offset += 64 * 1024) {
            const chunk = next.value.subarray(offset, offset + 64 * 1024);
            const credit = new Promise<void>((resolve, reject) => { acknowledge = resolve; rejectAcknowledgement = reject; });
            // This bounded chunk avoids both argument-stack and unbounded
            // socket-queue growth for the server's largest permitted file.
            let binary = ''; for (const byte of chunk) binary += String.fromCharCode(byte);
            socket.send(JSON.stringify({ type: 'http_data', data: btoa(binary) }));
            await credit; acknowledge = undefined; rejectAcknowledgement = undefined;
          }
        }
        if (!complete) socket.send(JSON.stringify({ type: 'http_end' }));
      } catch (error) { fail(error); }
    };
    socket.onopen = () => {
      if (complete) { socket.close(); return; }
      socket.send(JSON.stringify({ type: 'http_request', method: request.method, path: route.path, headers: Object.fromEntries(request.headers) }));
    };
    socket.onmessage = event => {
      if (complete) return;
      try {
        const frame = JSON.parse(String(event.data));
        if (frame.type === 'request_ready' && !started) { started = true; void upload(); }
        else if (frame.type === 'request_ack') acknowledge?.();
        else if (frame.type === 'response' && Number.isInteger(frame.status) && frame.status >= 200 && frame.status <= 599) { status = frame.status; headers = frame.headers; }
        else if (frame.type === 'data' && typeof frame.data === 'string' && frame.data.length <= 100_000) {
          const binary = atob(frame.data); responseSize += binary.length;
          if (responseSize > 8 * 1024 * 1024) throw new Error('Upload response exceeded its limit');
          response.push(Uint8Array.from(binary, char => char.charCodeAt(0)));
        } else if (frame.type === 'end' && status) {
          const value = new Response([204, 205, 304].includes(status) ? null : new Blob(response), { status, headers });
          complete = true; rejectAcknowledgement?.(new Error('Upload completed')); cleanup(); resolve(value);
        } else if (frame.type === 'error') throw new Error('The host could not complete the upload. Check the server before retrying.');
      } catch (error) { fail(error); }
    };
    socket.onerror = () => fail(new Error('Could not connect to the public host.'));
    socket.onclose = () => fail(new Error('The upload connection closed. Check the server before retrying.'));
  });
}
