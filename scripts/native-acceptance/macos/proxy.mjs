import fs from 'node:fs';
import https from 'node:https';
import http from 'node:http';
import { createHash, X509Certificate } from 'node:crypto';
import path from 'node:path';

const [configPath] = process.argv.slice(2);
const cfg = JSON.parse(fs.readFileSync(configPath, 'utf8').replace(/^\uFEFF/, ''));
if (cfg.listenHost !== '127.0.0.1' || cfg.listenPort !== 3480 || cfg.upstreamPort !== 3080 || cfg.hostname !== 'annex-native.localtest.me') throw new Error('Unexpected isolated guest fixture boundary');
const sockets = new Set();
const stats = { requests: 0, upgrades: 0, upstream_errors: 0 };
const cert = fs.readFileSync(cfg.certificate);
const spki = createHash('sha256').update(new X509Certificate(cert).publicKey.export({ type: 'spki', format: 'der' })).digest('base64');
const validHost = request => request.headers.host === `${cfg.hostname}:${cfg.listenPort}`;
function track(socket) {
  if (sockets.size >= 128) { socket.destroy(); return; }
  sockets.add(socket);
  socket.setTimeout(120_000, () => socket.destroy());
  socket.once('close', () => sockets.delete(socket));
  socket.on('error', () => {});
}
function options(request) {
  return { hostname: '127.0.0.1', port: cfg.upstreamPort, method: request.method,
    path: request.url, headers: { ...request.headers, 'x-forwarded-proto': 'https', 'x-forwarded-host': `${cfg.hostname}:${cfg.listenPort}` }, agent: false };
}
const credentials = cfg.pfx ? { pfx: fs.readFileSync(cfg.pfx), passphrase: cfg.passphrase } : { key: fs.readFileSync(cfg.privateKey), cert };
const server = https.createServer({ ...credentials, minVersion: 'TLSv1.2' }, (request, response) => {
  if (!validHost(request)) { response.writeHead(421).end(); return; }
  stats.requests++;
  const upstream = http.request(options(request), remote => {
    // Passive recording of response status and genuine invite creation only.
    // No request/authentication bodies or session tokens are retained.
    const pathname = new URL(request.url, 'http://127.0.0.1').pathname;
    if (cfg.passiveEvidenceDirectory && request.method === 'POST' && /^\/api\/(invites(?:\/redeem)?|registry\/register|zk\/verify-membership|session\/refresh)$/.test(pathname)) {
      const event = {at:new Date().toISOString(),method:request.method,path:pathname,status:remote.statusCode,request_origin:request.headers.origin ?? null};
      fs.appendFileSync(path.join(cfg.passiveEvidenceDirectory,'http-observations.jsonl'),JSON.stringify(event)+'\n');
      if (pathname === '/api/invites' && remote.statusCode >= 200 && remote.statusCode < 300) {
        const chunks=[]; let bytes=0;
        remote.on('data',chunk=>{bytes+=chunk.length;if(bytes<=65536)chunks.push(chunk)});
        remote.on('end',()=>{if(bytes<=65536)fs.appendFileSync(path.join(cfg.passiveEvidenceDirectory,'created-invites-private.jsonl'),JSON.stringify({...event,response:Buffer.concat(chunks).toString('utf8'),passive:true})+'\n')});
      }
    }
    response.writeHead(remote.statusCode, remote.headers);
    remote.pipe(response);
    remote.on('error', () => response.destroy());
  });
  upstream.setTimeout(120_000, () => upstream.destroy(new Error('upstream timeout')));
  upstream.on('error', () => { stats.upstream_errors++; if (!response.headersSent) response.writeHead(502); response.end(); });
  request.once('aborted', () => upstream.destroy());
  response.once('close', () => upstream.destroy());
  request.pipe(upstream);
});
server.maxHeadersCount = 100;
server.headersTimeout = 30_000;
server.requestTimeout = 120_000;
server.on('connection', track);
server.on('clientError', (_error, socket) => socket.destroy());
server.on('upgrade', (request, socket, head) => {
  if (!validHost(request) || request.headers.upgrade?.toLowerCase() !== 'websocket') { socket.end('HTTP/1.1 400 Bad Request\r\nConnection: close\r\n\r\n'); return; }
  const upstream = http.request(options(request));
  upstream.setTimeout(5_000, () => upstream.destroy(new Error('upgrade timeout')));
  upstream.once('upgrade', (response, remote, remoteHead) => {
    stats.upgrades++;
    track(remote);
    socket.write(`HTTP/1.1 ${response.statusCode} ${response.statusMessage}\r\n${response.rawHeaders.reduce((lines, field, index, fields) => index % 2 === 0 ? lines + `${field}: ${fields[index + 1]}\r\n` : lines, '')}\r\n`);
    if (remoteHead.length) socket.write(remoteHead);
    if (head.length) remote.write(head);
    socket.pipe(remote); remote.pipe(socket);
    socket.once('close', () => remote.destroy()); remote.once('close', () => socket.destroy());
  });
  upstream.once('response', response => { socket.end(`HTTP/1.1 ${response.statusCode} ${response.statusMessage}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n`); response.resume(); });
  upstream.once('error', () => { stats.upstream_errors++; socket.destroy(); });
  socket.once('close', () => upstream.destroy());
  upstream.end();
});
server.listen(cfg.listenPort, cfg.listenHost, () => {
  fs.writeFileSync(cfg.runtimeReceipt, JSON.stringify({ pid: process.pid, started_at: new Date().toISOString(), listen: `${cfg.listenHost}:${cfg.listenPort}`, upstream: `127.0.0.1:${cfg.upstreamPort}`, hostname: cfg.hostname, certificate_spki_sha256_base64: spki, proxy_sha256: createHash('sha256').update(fs.readFileSync(new URL(import.meta.url))).digest('hex') }, null, 2));
  console.log(JSON.stringify({ ok: true, listening: `${cfg.listenHost}:${cfg.listenPort}`, spki }));
});
let closing = false;
function close() {
  if (closing) return; closing = true;
  for (const socket of sockets) socket.destroy();
  server.close(() => { console.log(JSON.stringify({ stopped: true, stats })); process.exit(0); });
  setTimeout(() => process.exit(1), 2000).unref();
}
process.on('SIGTERM', close); process.on('SIGINT', close);
