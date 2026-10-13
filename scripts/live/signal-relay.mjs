// Serve the repository's actual signaling handler on a real TCP listener.
// Usage: node scripts/live/signal-relay.mjs <public trust-map JSON> [port]
// This adapter supplies the HTTP request/response shape used by the deployment
// handler; it never fabricates signaling responses or inspects sealed SDP.
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { createServer } from 'node:http';
import handler from '../../api/signal.js';

const [trustFile, portText = '3030'] = process.argv.slice(2);
assert(trustFile, 'Provide a file containing only the authorized public peer keys');
const trust = JSON.parse(readFileSync(trustFile, 'utf8').replace(/^\uFEFF/, ''));
assert(trust && !Array.isArray(trust) && Object.keys(trust).length > 0);
for (const [slug, publicKey] of Object.entries(trust)) {
  assert(/^[a-f0-9]{12}$/.test(slug), 'Trust entries require real 12-hex server slugs');
  assert(/^[a-f0-9]{64}$/.test(publicKey), 'Trust entries require Ed25519 public keys');
}
const port = Number(portText);
assert(Number.isSafeInteger(port) && port > 0 && port <= 65535);
process.env.ANNEX_BUILD_PROFILE = 'production';
process.env.ANNEX_SIGNAL_TRUSTED_PEERS = JSON.stringify(trust);

const server = createServer(async (request, response) => {
  const url = new URL(request.url, 'http://localhost');
  if (url.pathname !== '/api/signal') { response.writeHead(404).end(); return; }
  const started = Date.now();
  let bytes = 0;
  let body;
  try {
    const chunks = [];
    for await (const chunk of request) {
      bytes += chunk.length;
      if (bytes > 1024 * 1024) { response.writeHead(413).end(); return; }
      chunks.push(chunk);
    }
    if (bytes) body = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  } catch { response.writeHead(400).end(); return; }
  request.query = Object.fromEntries(url.searchParams);
  request.body = body;
  response.status = code => { response.statusCode = code; return response; };
  response.json = value => {
    response.setHeader('content-type', 'application/json; charset=utf-8');
    response.end(JSON.stringify(value));
  };
  response.on('finish', () => console.log(JSON.stringify({
    type: 'signal_request', at: new Date().toISOString(),
    method: request.method, status: response.statusCode, bytes,
    elapsedMs: Date.now() - started,
    format: body?.format_version ?? null, sdpType: body?.sdp_type ?? null,
    // Public structural observations only: no keys, tags, signatures or SDP.
    hasStableSlug: Boolean(body?.from_server_slug || body?.to_server_slug),
    hasRendezvousTag: Boolean(body?.rendezvous_tag),
  })));
  try { await handler(request, response); }
  catch (error) {
    console.error(JSON.stringify({ type: 'relay_error', message: error.message }));
    if (!response.headersSent) response.writeHead(500);
    response.end();
  }
});
server.listen(port, '0.0.0.0', () => console.log(JSON.stringify({
  type: 'ready', port, productionAuthentication: true, authorizedPeers: Object.keys(trust).length,
})));
for (const signal of ['SIGINT', 'SIGTERM']) process.on(signal, () => {
  server.close(() => process.exit(0));
  server.closeAllConnections();
});
