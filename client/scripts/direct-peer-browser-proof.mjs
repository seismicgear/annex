// Browser/native transport acceptance. The optional loopback remap changes
// only the introduction socket's network address; it never supplies responses.
import { createServer } from 'vite';
import { chromium } from 'playwright';
import { WebSocket } from 'ws';
import { createHash } from 'node:crypto';
import { fileURLToPath } from 'node:url';

const [publicUrl, introducer, control] = process.argv.slice(2);
const id = new URL(publicUrl).pathname.split('/')[2];
if (!/^[0-9a-f]{64}$/.test(id)) throw new Error('Expected native host identity');
for (const value of [introducer, control].filter(Boolean)) {
  const parsed = new URL(value); if (parsed.protocol !== 'http:' || parsed.hostname !== '127.0.0.1') throw new Error('Fixture remapping is loopback only');
}
const vite = await createServer({ configFile: false, root: fileURLToPath(new URL('..', import.meta.url)), server: { host: '127.0.0.1', port: 0 } });
vite.middlewares.use('/direct-proof', (_req, res) => { res.setHeader('Content-Type', 'text/html'); res.end('<!doctype html><title>Annex direct transport acceptance</title><h1>Annex direct transport acceptance</h1>'); });
await vite.listen();
const browser = await chromium.launch({ headless: true, executablePath: process.env.ANNEX_TEST_BROWSER_EXECUTABLE || undefined });
try {
  const page = await browser.newPage();
  let introductions = 0; const signaling = [];
  let blockedIntroductions = 0;
  if (!introducer) page.on('websocket', socket => {
    if (!/^wss:\/\/www\.monolithannex\.com\/host\/[0-9a-f]{64}\/__tunnel$/.test(socket.url())) return;
    introductions++;
    socket.on('framesent', frame => signaling.push(JSON.parse(String(frame.payload)).type));
  });
  if (introducer) await page.routeWebSocket(/^wss:\/\/www\.monolithannex\.com\/host\/[0-9a-f]{64}\/__tunnel$/, route => {
    introductions++;
    const upstream = new WebSocket(introducer.replace('http:', 'ws:') + new URL(route.url()).pathname);
    const waiting = [];
    route.onMessage(message => {
      const type = JSON.parse(String(message)).type; signaling.push(type);
      if (!['tunnel_open', 'peer_offer'].includes(type)) throw new Error('Application frame reached introducer');
      if (upstream.readyState === WebSocket.OPEN) upstream.send(message); else waiting.push(message);
    });
    upstream.on('open', () => { for (const message of waiting.splice(0)) upstream.send(message); });
    upstream.on('message', bytes => route.send(bytes.toString()));
    upstream.on('error', () => route.close({ code: 1011, reason: 'Fixture introduction failed' }));
    upstream.on('close', (code, reason) => route.close({ code: code === 1006 ? 1011 : code, reason: reason.toString() }));
    route.onClose(() => upstream.close());
  });
  await page.goto(vite.resolvedUrls.local[0] + 'direct-proof');
  const first = await page.evaluate(async id => {
    const { transportFetch } = await import('/src/api/hosted-fetch.ts');
    const { transportSocket } = await import('/src/lib/host-tunnel.ts');
    const base = 'https://www.monolithannex.com/host/' + id;
    const summary = await (await transportFetch(base + '/api/public/server/summary')).json();
    const upload = await (await transportFetch(base + '/api/upload', { method: 'POST', body: new Uint8Array(1024 * 1024).fill(0x39), headers: { authorization: 'Bearer browser-fixture' } })).json();
    const socket = transportSocket(base.replace('https:', 'wss:') + '/ws');
    await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
    const echoed = new Promise((resolve, reject) => { socket.onmessage = e => resolve(e.data); socket.onerror = reject; });
    socket.send('browser direct session');
    window.__directProof = { socket, base, transportFetch };
    return { summary, upload, echo: await echoed };
  }, id);
  if (first.summary.slug !== 'native-target' || first.echo !== 'browser direct session'
      || first.upload.bytes !== 1024 * 1024 || first.upload.sha256 !== createHash('sha256').update(Buffer.alloc(1024 * 1024, 0x39)).digest('hex')) throw new Error('Browser/native payload mismatch');
  const before = introductions;
  if (!introducer) await page.routeWebSocket(/^wss:\/\/www\.monolithannex\.com\/host\/[0-9a-f]{64}\/__tunnel$/, route => {
    blockedIntroductions++;
    route.close({ code: 1011, reason: 'Introducer deliberately unavailable after direct handoff' });
  });
  if (control) {
    const stopped = await (await fetch(control + '/stop', { method: 'POST' })).json();
    if (!stopped.stopped) throw new Error('Introducer did not stop');
  }
  if (control || !introducer) {
    const continued = await page.evaluate(async () => {
      const { socket, base, transportFetch } = window.__directProof;
      const echoed = new Promise(resolve => { socket.onmessage = e => resolve(e.data); });
      socket.send('browser without Monolith');
      const summary = await (await transportFetch(base + '/api/public/server/summary')).json();
      const bytes = new Uint8Array(await (await transportFetch(base + '/api/download')).arrayBuffer());
      return { summary, echo: await echoed, length: bytes.length, correctBytes: bytes.every(b => b === 0x6b) };
    });
    if (continued.summary.slug !== 'native-target' || continued.echo !== 'browser without Monolith'
      || continued.length !== 1024 * 1024 || !continued.correctBytes || introductions !== before || blockedIntroductions !== 0) throw new Error('Browser still depends on introducer');
  }
  if (introductions !== 1 || signaling.join(',') !== 'tunnel_open,peer_offer') throw new Error('A live browser peer repeated introduction or relayed data');
  console.log(JSON.stringify({ pass: true, browser: 'Chromium', browserVersion: browser.version(), nativeHost: id, introductionSocketRemappedToLoopback: !!introducer,
    productionServiceAccepted: !introducer, introductions, signalingFrames: signaling, bytesVerified: 1024 * 1024,
    establishedSocketAndNewHttpAfterIntroducerShutdown: !!control,
    establishedSocketAndNewHttpWithFurtherIntroductionsBlocked: !introducer,
    fixtureApplicationBackend: true }));
} finally { await browser.close(); await vite.close(); }
