// Independent read-only probe. Node/OpenSSL owns the cipher; Chromium supplies
// only WebRTC. No application imports, identities, HTTP relay or TURN fallback.
import { createHash, createPublicKey, generateKeyPairSync, randomBytes, verify, diffieHellman, hkdfSync, createCipheriv, createDecipheriv } from 'node:crypto';
import { createRequire } from 'node:module';
const { chromium } = createRequire(new URL('../../client/package.json', import.meta.url))('playwright');
const origin = process.argv[2];
if (!/^https:\/\/www\.monolithannex\.com\/host\/[0-9a-f]{64}$/.test(origin || '')) throw new Error('Expected canonical public host address');
const host = origin.split('/').pop(), domain = Buffer.from('annex-content-blind-v2');
const sha = bytes => createHash('sha256').update(bytes).digest();
const publicKey = (raw, algorithm) => createPublicKey({ key: Buffer.concat([Buffer.from(algorithm === 'ed25519' ? '302a300506032b6570032100' : '302a300506032b656e032100', 'hex'), raw]), format: 'der', type: 'spki' });
const decode = value => {
  if (typeof value !== 'string') throw new Error('Missing encoded bytes');
  const bytes = Buffer.from(value, 'base64'); if (bytes.toString('base64') !== value) throw new Error('Noncanonical base64'); return bytes;
};
function cipher() {
  const keys = generateKeyPairSync('x25519');
  const hello = Buffer.concat([keys.publicKey.export({ format: 'der', type: 'spki' }).subarray(-32), randomBytes(32)]);
  let tx, rx, digest, received = 0, sent = 0, fragments = [], total = 0;
  return {
    hello: hello.toString('base64'),
    finish(value) {
      const welcome = decode(value);
      if (welcome.length !== 128 || sha(welcome.subarray(0, 32)).toString('hex') !== host) throw new Error('Wrong host key');
      const transcript = Buffer.concat([domain, welcome.subarray(0, 32), hello, welcome.subarray(32, 64)]);
      if (!verify(null, transcript, publicKey(welcome.subarray(0, 32), 'ed25519'), welcome.subarray(64))) throw new Error('Host signature failed');
      digest = sha(transcript);
      const shared = diffieHellman({ privateKey: keys.privateKey, publicKey: publicKey(welcome.subarray(32, 64), 'x25519') });
      tx = Buffer.from(hkdfSync('sha256', shared, digest, Buffer.concat([domain, Buffer.from('/client-to-host')]), 32));
      rx = Buffer.from(hkdfSync('sha256', shared, digest, Buffer.concat([domain, Buffer.from('/host-to-client')]), 32)); shared.fill(0);
    },
    seal(value) {
      const request = Buffer.from(JSON.stringify(value));
      if (request.length > 48 * 1024 || sent > 0xffffffff) throw new Error('Probe packet exceeds bound');
      const plain = Buffer.alloc(Math.ceil((request.length + 5) / 256) * 256); plain[0] = 1; plain.writeUInt32BE(request.length, 1); request.copy(plain, 5);
      const sequence = sent++, nonce = Buffer.alloc(12); nonce.writeUInt32BE(sequence, 8);
      const encoder = createCipheriv('chacha20-poly1305', tx, nonce, { authTagLength: 16 });
      encoder.setAAD(Buffer.concat([digest, nonce]), { plaintextLength: plain.length });
      const data = Buffer.concat([encoder.update(plain), encoder.final(), encoder.getAuthTag()]); plain.fill(0);
      return { sequence, data: data.toString('base64') };
    },
    open(frame) {
      if (frame.sequence !== received || received > 0xffffffff) throw new Error('Replayed or missing packet');
      const sealed = decode(frame.data); if (sealed.length < 272 || sealed.length > 48 * 1024 + 272) throw new Error('Invalid encrypted length');
      const nonce = Buffer.alloc(12); nonce.writeUInt32BE(received++, 8);
      const decoder = createDecipheriv('chacha20-poly1305', rx, nonce, { authTagLength: 16 });
      decoder.setAuthTag(sealed.subarray(-16)); decoder.setAAD(Buffer.concat([digest, nonce]), { plaintextLength: sealed.length - 16 });
      const plain = Buffer.concat([decoder.update(sealed.subarray(0, -16)), decoder.final()]);
      const length = plain.readUInt32BE(1), last = plain[0];
      if (last > 1 || length > 48 * 1024 || length + 5 > plain.length || plain.length % 256 || plain.subarray(length + 5).some(byte => byte)) throw new Error('Invalid encrypted framing');
      fragments.push(Buffer.from(plain.subarray(5, length + 5))); total += length; plain.fill(0);
      if (total > 1024 * 1024) throw new Error('Summary exceeded bound');
      if (!last) return undefined;
      const value = JSON.parse(Buffer.concat(fragments).toString('utf8')); fragments = []; total = 0; return value;
    },
    destroy() { tx?.fill(0); rx?.fill(0); },
  };
}
function mailbox() {
  const pending = []; let waiting, failure;
  return {
    push(raw) {
      if (typeof raw !== 'string' || raw.length > 128 * 1024 || pending.length >= 64) { this.fail(new Error('Invalid probe packet')); return; }
      try { const value = JSON.parse(raw); if (waiting) { const next = waiting; waiting = null; next.resolve(value); } else pending.push(value); }
      catch (error) { this.fail(error); }
    },
    fail(error) { failure = error; waiting?.reject(error); waiting = null; },
    async next() {
      if (pending.length) return pending.shift(); if (failure) throw failure;
      let timer;
      try { return await new Promise((resolve, reject) => { waiting = { resolve, reject }; timer = setTimeout(() => this.fail(new Error('Direct probe timed out')), 30_000); }); }
      finally { clearTimeout(timer); }
    },
  };
}
const browser = await chromium.launch({ headless: true, executablePath: process.env.ANNEX_TEST_BROWSER_EXECUTABLE || undefined });
const introduction = cipher(), application = cipher(); let socket, page;
try {
  page = await browser.newPage();
  const incoming = mailbox();
  await page.exposeFunction('probeReceive', raw => incoming.push(raw));
  await page.exposeFunction('probeClosed', () => incoming.fail(new Error('Direct application channel closed')));
  const offer = await page.evaluate(async () => {
    const pc = window.probePeer = new RTCPeerConnection({ iceServers: [{ urls: 'stun:stun.l.google.com:19302' }] });
    window.probeControl = pc.createDataChannel('annex-peer-v3');
    await pc.setLocalDescription(await pc.createOffer());
    await new Promise(resolve => {
      const finish = () => { clearTimeout(timer); pc.removeEventListener('icegatheringstatechange', changed); resolve(); };
      const changed = () => { if (pc.iceGatheringState === 'complete') finish(); };
      const timer = setTimeout(finish, 10_000); pc.addEventListener('icegatheringstatechange', changed); changed();
    });
    return pc.localDescription.sdp;
  });
  if (!offer.startsWith('v=0\r\n') || offer.length > 24 * 1024 || !/^a=candidate:.*\btyp (host|srflx)\b/m.test(offer)) throw new Error('No direct probe address');
  const signaling = mailbox();
  socket = new WebSocket(origin.replace('https:', 'wss:') + '/__tunnel');
  socket.onmessage = event => signaling.push(event.data);
  socket.onerror = () => signaling.fail(new Error('Public introduction failed'));
  socket.onclose = event => signaling.fail(new Error('Public introduction closed: ' + event.code));
  socket.onopen = () => socket.send(JSON.stringify({ type: 'tunnel_open', hello: introduction.hello }));
  const hello = await signaling.next(); if (hello.type !== 'tunnel_hello') throw new Error('Missing introduction handshake');
  introduction.finish(hello.hello);
  socket.send(JSON.stringify({ type: 'peer_offer', ...introduction.seal({ protocol: 'annex-direct-v3', type: 'offer', sdp: offer }) }));
  const packet = await signaling.next(); if (packet.type !== 'peer_answer') throw new Error('Missing direct answer');
  const answer = introduction.open(packet);
  if (answer?.protocol !== 'annex-direct-v3' || answer.type !== 'answer' || typeof answer.sdp !== 'string'
      || answer.sdp.length > 24 * 1024 || !answer.sdp.startsWith('v=0\r\n') || /^a=candidate:.*\btyp relay\b/m.test(answer.sdp)) throw new Error('Invalid direct answer');
  socket.close(1000, 'Peers introduced'); introduction.destroy();
  await page.evaluate(async answer => {
    const opened = channel => new Promise((resolve, reject) => {
      if (channel.readyState === 'open') return resolve();
      const timer = setTimeout(() => reject(new Error('Direct channel did not open')), 15_000);
      channel.onopen = () => { clearTimeout(timer); resolve(); };
      channel.onclose = () => { clearTimeout(timer); reject(new Error('Direct channel closed')); };
    });
    await window.probePeer.setRemoteDescription({ type: 'answer', sdp: answer.sdp });
    await opened(window.probeControl);
    const channel = window.probeApplication = window.probePeer.createDataChannel('annex-application-v3');
    channel.onmessage = event => window.probeReceive(event.data);
    await opened(channel); channel.onclose = () => window.probeClosed();
  }, answer);
  const send = frame => page.evaluate(raw => window.probeApplication.send(raw), JSON.stringify(frame));
  await send({ type: 'tunnel_open', hello: application.hello });
  const welcome = await incoming.next(); if (welcome.type !== 'tunnel_hello') throw new Error('Missing direct handshake');
  application.finish(welcome.hello);
  await send({ type: 'tunnel_data', ...application.seal({ type: 'http_open', method: 'GET', path: '/api/public/server/summary', headers: {} }) });
  let status = 0, bytes = 0; const body = [];
  for (;;) {
    const frame = await incoming.next(); if (frame.type !== 'tunnel_data') throw new Error('Unexpected direct packet');
    const value = application.open(frame); if (!value) continue;
    if (value.type === 'request_ready') await send({ type: 'tunnel_data', ...application.seal({ type: 'http_end' }) });
    else if (value.type === 'response' && status === 0) status = value.status;
    else if (value.type === 'data' && status === 200) {
      const chunk = decode(value.data); bytes += chunk.length; if (bytes > 1024 * 1024) throw new Error('Summary too large'); body.push(chunk);
      await send({ type: 'tunnel_data', ...application.seal({ type: 'response_ack' }) });
    } else if (value.type === 'end' && status === 200) { console.log(JSON.stringify(JSON.parse(Buffer.concat(body).toString('utf8')))); break; }
    else throw new Error('Host did not return its public summary');
  }
} catch (error) {
  // Keep the actual ICE outcome when a different network cannot connect.
  // No SDP credentials, invitation code, host address or packet content is logged.
  const connectivity = await page?.evaluate(async () => {
    const pc = window.probePeer;
    if (!pc) return { phase: 'before-peer' };
    const candidates = description => (description?.sdp ?? '').split(/\r?\n/)
      .filter(line => line.startsWith('a=candidate:'))
      .map(line => { const words = line.split(/\s+/); const type = words.indexOf('typ'); return { protocol: words[2], type: words[type + 1], family: words[4].includes(':') ? 'IPv6' : 'IPv4', port: Number(words[5]) }; });
    const pairs = [];
    for (const row of (await pc.getStats()).values()) {
      if (row.type === 'candidate-pair') pairs.push({ state: row.state, nominated: row.nominated,
        requestsSent: row.requestsSent, responsesReceived: row.responsesReceived,
        requestsReceived: row.requestsReceived, responsesSent: row.responsesSent,
        bytesSent: row.bytesSent, bytesReceived: row.bytesReceived });
    }
    return { connection: pc.connectionState, ice: pc.iceConnectionState, gathering: pc.iceGatheringState,
      signaling: pc.signalingState, local: candidates(pc.localDescription), remote: candidates(pc.remoteDescription), pairs };
  }).catch(() => ({ phase: 'diagnostic-unavailable' }));
  console.error('Direct connectivity observation: ' + JSON.stringify(connectivity));
  throw error;
} finally { socket?.close(); introduction.destroy(); application.destroy(); await browser.close(); }
