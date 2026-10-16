// Independent read-only acceptance probe using Node/OpenSSL, not the app's
// cipher implementation. It verifies the pinned server and reads its public
// summary over the actual encrypted route. It never provisions an identity.
import { createHash, createPublicKey, generateKeyPairSync, randomBytes, verify, diffieHellman, hkdfSync, createCipheriv, createDecipheriv } from 'node:crypto';
const origin = process.argv[2];
if (!/^https:\/\/www\.monolithannex\.com\/host\/[0-9a-f]{64}$/.test(origin || '')) throw new Error('Expected the canonical public host address');
const host = origin.split('/').pop(), domain = Buffer.from('annex-content-blind-v2');
const sha = bytes => createHash('sha256').update(bytes).digest();
const publicKey = (raw, algorithm) => createPublicKey({ key: Buffer.concat([Buffer.from(algorithm === 'ed25519' ? '302a300506032b6570032100' : '302a300506032b656e032100', 'hex'), raw]), format: 'der', type: 'spki' });
const decode = value => {
  if (typeof value !== 'string') throw new Error('Missing encoded bytes');
  const bytes = Buffer.from(value, 'base64'); if (bytes.toString('base64') !== value) throw new Error('Noncanonical base64'); return bytes;
};
const keys = generateKeyPairSync('x25519');
const hello = Buffer.concat([keys.publicKey.export({ format: 'der', type: 'spki' }).subarray(-32), randomBytes(32)]);
let tx, rx, digest, received = 0, fragments = [], total = 0, status = 0, body = [];
const summary = await new Promise((resolve, reject) => {
  const socket = new WebSocket(origin.replace('https:', 'wss:') + '/__tunnel');
  let finished = false;
  const end = (error, value) => { if (finished) return; finished = true; clearTimeout(timer); socket.close(); tx?.fill(0); rx?.fill(0); error ? reject(error) : resolve(value); };
  const timer = setTimeout(() => end(new Error('Encrypted public summary timed out')), 35_000);
  socket.onerror = () => end(new Error('Public host WebSocket failed'));
  socket.onclose = event => end(new Error('Public host closed: ' + event.code));
  socket.onopen = () => socket.send(JSON.stringify({ type: 'tunnel_open', hello: hello.toString('base64') }));
  socket.onmessage = event => {
    try {
      if (typeof event.data !== 'string' || event.data.length > 128 * 1024) throw new Error('Invalid outer packet');
      const frame = JSON.parse(event.data);
      if (!rx) {
        if (frame.type !== 'tunnel_hello') throw new Error('Missing authenticated handshake');
        const welcome = decode(frame.hello);
        if (welcome.length !== 128 || sha(welcome.subarray(0, 32)).toString('hex') !== host) throw new Error('Wrong host key');
        const transcript = Buffer.concat([domain, welcome.subarray(0, 32), hello, welcome.subarray(32, 64)]);
        if (!verify(null, transcript, publicKey(welcome.subarray(0, 32), 'ed25519'), welcome.subarray(64))) throw new Error('Host signature failed');
        digest = sha(transcript);
        const shared = diffieHellman({ privateKey: keys.privateKey, publicKey: publicKey(welcome.subarray(32, 64), 'x25519') });
        tx = Buffer.from(hkdfSync('sha256', shared, digest, Buffer.concat([domain, Buffer.from('/client-to-host')]), 32));
        rx = Buffer.from(hkdfSync('sha256', shared, digest, Buffer.concat([domain, Buffer.from('/host-to-client')]), 32)); shared.fill(0);
        const request = Buffer.from(JSON.stringify({ type: 'http', method: 'GET', path: '/api/public/server/summary', headers: {}, body: '' }));
        const plain = Buffer.alloc(Math.ceil((request.length + 5) / 256) * 256); plain[0] = 1; plain.writeUInt32BE(request.length, 1); request.copy(plain, 5);
        const nonce = Buffer.alloc(12), cipher = createCipheriv('chacha20-poly1305', tx, nonce, { authTagLength: 16 });
        cipher.setAAD(Buffer.concat([digest, nonce]), { plaintextLength: plain.length });
        const data = Buffer.concat([cipher.update(plain), cipher.final(), cipher.getAuthTag()]); plain.fill(0);
        socket.send(JSON.stringify({ type: 'tunnel_data', sequence: 0, data: data.toString('base64') })); return;
      }
      if (frame.type !== 'tunnel_data' || frame.sequence !== received || received > 0xffffffff) throw new Error('Replayed or missing encrypted packet');
      const sealed = decode(frame.data); if (sealed.length < 272 || sealed.length > 48 * 1024 + 272) throw new Error('Invalid encrypted length');
      const nonce = Buffer.alloc(12); nonce.writeUInt32BE(received++, 8);
      const cipher = createDecipheriv('chacha20-poly1305', rx, nonce, { authTagLength: 16 });
      cipher.setAuthTag(sealed.subarray(-16)); cipher.setAAD(Buffer.concat([digest, nonce]), { plaintextLength: sealed.length - 16 });
      const plain = Buffer.concat([cipher.update(sealed.subarray(0, -16)), cipher.final()]);
      const length = plain.readUInt32BE(1), last = plain[0];
      if (last > 1 || length > 48 * 1024 || length + 5 > plain.length || plain.length % 256 || plain.subarray(length + 5).some(byte => byte)) throw new Error('Invalid encrypted framing');
      fragments.push(Buffer.from(plain.subarray(5, length + 5))); total += length; plain.fill(0);
      if (total > 1024 * 1024) throw new Error('Public summary exceeded its limit');
      if (!last) return;
      const value = JSON.parse(Buffer.concat(fragments).toString('utf8')); fragments = [];
      if (value.type === 'response' && status === 0) status = value.status;
      else if (value.type === 'data' && status === 200) body.push(decode(value.data));
      else if (value.type === 'end' && status === 200) end(null, JSON.parse(Buffer.concat(body).toString('utf8')));
      else throw new Error('Host did not return its public summary');
    } catch (error) { end(error); }
  };
});
console.log(JSON.stringify(summary));
