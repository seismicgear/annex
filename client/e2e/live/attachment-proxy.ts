import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createServer, request as httpRequest } from 'node:http';
import { createConnection, type Socket } from 'node:net';
import { Transform } from 'node:stream';

const MAX_BODY = 64 * 1024 * 1024;
const MAX_RETAINED = 128 * 1024 * 1024;
const MAX_UPLOADS = 256;
type Capture = {
  index: number; path: string; contentType: string; chunks: Buffer[];
  bytes: number; complete: boolean; interrupted: boolean; attachmentId?: string;
  forwardedBytes: number; controlledFault?: 'partial-body-hold' | 'response-hold' | 'persistent-refusal';
  upstreamResponseStatus?: number;
};
type Fault = {
  path: string; mode: 'body' | 'response'; held: boolean; released: boolean;
  capture?: Capture; resume?: () => void; disconnect?: () => void;
  disconnected?: boolean;
};

/** Passive observation of actual encrypted-upload entity bytes. All HTTP
 * bodies and responses stream through unchanged; upgrades use a raw TCP pipe.
 * Only ciphertext multipart uploads are retained, bounded and in memory. No
 * credentials, identity proofs, message bodies or downloaded plaintext are
 * recorded. Forwarding errors close the transport, never fabricate replies. */
export async function attachmentProxy(backendURL: string, browserURL: string) {
  const backend = new URL(backendURL), browser = new URL(browserURL);
  for (const url of [backend, browser]) {
    assert(url.protocol === 'http:' && url.hostname === '127.0.0.1' && url.port
      && url.pathname === '/' && !url.search && !url.hash && !url.username && !url.password,
    'Attachment evidence forwarding requires two explicit loopback HTTP origins');
  }
  assert.notEqual(backend.origin, browser.origin);
  const captures: Capture[] = [];
  const blockedUploadPaths = new Set<string>();
  let armedFault: Fault | undefined;
  const sockets = new Set<Socket>();
  let retained = 0, observationFailure: string | undefined;
  const remember = (socket: Socket) => {
    sockets.add(socket); socket.once('close', () => sockets.delete(socket));
    return socket;
  };
  const observe = (capture: Capture, data: Buffer) => {
    if (observationFailure) return;
    if (capture.bytes + data.length > MAX_BODY || retained + data.length > MAX_RETAINED) {
      observationFailure = 'Passive upload evidence exceeded its explicit memory bound';
      return;
    }
    capture.chunks.push(Buffer.from(data)); capture.bytes += data.length; retained += data.length;
    if (!capture.attachmentId) {
      // The product places the opaque UUID before the file. An interrupted
      // body need not contain the full ciphertext to identify its real upload.
      const prefix = Buffer.concat(capture.chunks, capture.bytes).subarray(0, 16 * 1024).toString('latin1');
      const match = /name="attachment_id"\r\n\r\n([0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12})\r\n/.exec(prefix);
      if (match) capture.attachmentId = match[1];
      else if (capture.bytes > 16 * 1024) observationFailure = 'Actual upload omitted the bounded opaque UUID prefix';
    }
  };
  const server = createServer((incoming, outgoing) => {
    const path = new URL(incoming.url ?? '/', browser).pathname;
    let capture: Capture | undefined;
    if (incoming.method === 'POST' && /^\/api\/channels\/[^/]+\/encrypted-upload$/.test(path)) {
      if (captures.length >= MAX_UPLOADS) observationFailure = 'Passive upload evidence exceeded its request-count bound';
      else {
        capture = { index: captures.length, path, contentType: incoming.headers['content-type'] ?? '',
          chunks: [], bytes: 0, forwardedBytes: 0, complete: false, interrupted: false };
        captures.push(capture);
      }
    }
    if (capture && blockedUploadPaths.has(path)) {
      // Chromium can replay a POST whose connection closed before any reply.
      // Keep the real link down until the test explicitly restores it. Observe
      // only its actual opaque prefix; none of these replay bytes reaches the
      // backend, and no fabricated HTTP response is returned.
      const refused = capture; refused.controlledFault = 'persistent-refusal';
      incoming.on('data', (chunk: Buffer) => {
        observe(refused, chunk);
        if (refused.attachmentId || refused.bytes >= 16 * 1024) {
          incoming.destroy(); outgoing.destroy();
        }
      });
      incoming.on('aborted', () => { refused.interrupted = true; });
      incoming.on('error', () => { refused.interrupted = true; });
      incoming.on('end', () => { refused.complete = true; outgoing.destroy(); });
      return;
    }
    const fault = capture && armedFault?.path === path ? armedFault : undefined;
    if (fault) { armedFault = undefined; fault.capture = capture; }
    const upstream = httpRequest({ hostname: backend.hostname, port: Number(backend.port),
      path: incoming.url, method: incoming.method, headers: incoming.headers, agent: false }, response => {
      response.on('error', () => outgoing.destroy());
      if (capture) capture.upstreamResponseStatus = response.statusCode;
      const forward = () => {
        if (outgoing.destroyed) { response.destroy(); return; }
        outgoing.writeHead(response.statusCode!, response.statusMessage, response.rawHeaders);
        response.pipe(outgoing);
      };
      if (fault?.mode === 'response' && !fault.released) {
        // The real application has generated this response. Withhold it as a
        // stalled network would, without changing a header, status or byte.
        response.pause(); fault.held = true; capture!.controlledFault = 'response-hold';
        fault.resume = forward;
        outgoing.once('close', () => response.destroy());
      } else forward();
    });
    if (fault) fault.disconnect = () => { incoming.destroy(); outgoing.destroy(); upstream.destroy(); };
    upstream.on('socket', remember);
    upstream.on('error', () => outgoing.destroy());
    outgoing.on('close', () => { if (!outgoing.writableFinished) upstream.destroy(); });
    incoming.on('aborted', () => { if (capture) capture.interrupted = true; upstream.destroy(); });
    incoming.on('error', () => { if (capture) capture.interrupted = true; upstream.destroy(); });
    if (capture) {
      const observed = capture;
      const tap = new Transform({ transform(chunk: Buffer, _encoding, callback) {
        observe(observed, chunk);
        if (fault?.mode === 'body' && !fault.released && observed.forwardedBytes + chunk.length > 8192) {
          // Send a genuine bounded prefix and some ciphertext to the real
          // server, then apply backpressure to the remaining body. The test
          // can cut these sockets or invoke the actual UI cancel action.
          const count = Math.max(0, 8192 - observed.forwardedBytes);
          if (count) { this.push(chunk.subarray(0, count)); observed.forwardedBytes += count; }
          fault.held = true; observed.controlledFault = 'partial-body-hold';
          fault.resume = () => {
            const remainder = chunk.subarray(count); observed.forwardedBytes += remainder.length;
            callback(null, remainder);
          };
        } else { observed.forwardedBytes += chunk.length; callback(null, chunk); }
      } });
      incoming.on('end', () => { observed.complete = true; });
      incoming.on('close', () => { if (!incoming.complete) tap.destroy(); });
      tap.on('error', () => upstream.destroy());
      incoming.pipe(tap).pipe(upstream);
    } else incoming.pipe(upstream);
  });
  server.on('connection', remember);
  server.on('clientError', (_error, socket) => socket.destroy());
  server.on('upgrade', (incoming, client, head) => {
    const upstream = remember(createConnection({ host: backend.hostname, port: Number(backend.port) }));
    client.on('error', () => upstream.destroy()); upstream.on('error', () => client.destroy());
    client.on('close', () => upstream.destroy()); upstream.on('close', () => client.destroy());
    upstream.once('connect', () => {
      let headers = `${incoming.method} ${incoming.url} HTTP/${incoming.httpVersion}\r\n`;
      for (let i = 0; i < incoming.rawHeaders.length; i += 2) headers += `${incoming.rawHeaders[i]}: ${incoming.rawHeaders[i + 1]}\r\n`;
      upstream.write(`${headers}\r\n`);
      if (head.length) upstream.write(head);
      client.pipe(upstream); upstream.pipe(client);
    });
  });
  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(Number(browser.port), '127.0.0.1', () => { server.off('error', reject); resolve(); });
  });
  const healthy = () => assert(!observationFailure, observationFailure ?? 'Passive evidence is available');
  return {
    browserOrigin: browser.origin, backendOrigin: backend.origin,
    nextIndex: () => captures.length,
    holdNextUpload(path: string, mode: 'body' | 'response') {
      assert(!armedFault, 'Only one next-upload transport fault may be armed');
      assert(/^\/api\/channels\/[^/]+\/encrypted-upload$/.test(path));
      const fault: Fault = { path, mode, held: false, released: false };
      armedFault = fault;
      return {
        async waitUntilHeld() {
          const deadline = Date.now() + 30_000;
          while (Date.now() < deadline) {
            healthy();
            if (fault.held) {
              assert(fault.capture?.attachmentId, 'The actual multipart UUID must precede the held transfer');
              if (mode === 'body') assert.equal(fault.capture.forwardedBytes, 8192);
              else assert(fault.capture.complete && fault.capture.upstreamResponseStatus,
                'Acknowledgement hold requires the real completed request and upstream response');
              return fault.capture.attachmentId;
            }
            assert(!fault.capture?.interrupted, 'The request ended before the controlled hold boundary');
            await new Promise(resolve => setTimeout(resolve, 25));
          }
          throw new Error('The actual upload did not reach the requested transport hold boundary');
        },
        cut() {
          assert(fault.held && fault.disconnect, 'Only an actually held request can be cut');
          blockedUploadPaths.add(path); fault.disconnected = true; fault.disconnect();
        },
        release() {
          blockedUploadPaths.delete(path); fault.released = true;
          if (armedFault === fault) armedFault = undefined;
          if (!fault.disconnected) fault.resume?.(); fault.resume = undefined;
        },
      };
    },
    async waitForMetadata(afterIndex: number, path: string) {
      const deadline = Date.now() + 30_000;
      while (Date.now() < deadline) {
        healthy();
        const matches = captures.filter(item => item.index >= afterIndex && item.path === path && item.attachmentId);
        assert(matches.length <= 1, 'An upload action must produce one independently observed request');
        if (matches.length) return matches[0].attachmentId!;
        await new Promise(resolve => setTimeout(resolve, 25));
      }
      throw new Error('No actual encrypted-upload UUID reached the passive loopback observer');
    },
    body(attachmentId: string) {
      healthy();
      const matches = captures.filter(item => item.attachmentId === attachmentId);
      assert.equal(matches.length, 1, 'Exactly one actual upload must bind this UUID');
      const capture = matches[0];
      assert(capture.complete && !capture.interrupted, 'Complete multipart evidence requires a fully received request');
      return { contentType: capture.contentType, raw: Buffer.concat(capture.chunks, capture.bytes) };
    },
    publicReceipt() {
      healthy();
      return { source: 'passive-loopback-http-forwarder', browserOrigin: browser.origin, backendOrigin: backend.origin,
        captures: captures.map(capture => ({ uploadId: capture.attachmentId, bytesObserved: capture.bytes,
          forwardedBytes: capture.forwardedBytes, controlledFault: capture.controlledFault,
          upstreamResponseStatus: capture.upstreamResponseStatus,
          complete: capture.complete, interrupted: capture.interrupted,
          multipartSha256: createHash('sha256').update(Buffer.concat(capture.chunks, capture.bytes)).digest('hex') })) };
    },
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise<void>((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
      for (const capture of captures) { for (const chunk of capture.chunks) chunk.fill(0); capture.chunks.length = 0; }
    },
  };
}
