import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { ed25519, x25519 } from '@noble/curves/ed25519.js';
import { sha256 } from '@noble/hashes/sha2.js';
import { concatBytes, hexToBytes, utf8ToBytes } from '@noble/hashes/utils.js';
import { clientHello, fromBase64, toBase64, TunnelCipher } from './host-tunnel-crypto';
const fixture = JSON.parse(readFileSync(resolve('../docs/protocol/fixtures/public-host-v2.json'), 'utf8'));
function pair() {
  const hello = clientHello(hexToBytes(fixture.clientSeed), hexToBytes(fixture.nonce));
  const client = hello.finish(fixture.host, fixture.welcome);
  const welcome = fromBase64(fixture.welcome);
  const transcript = concatBytes(utf8ToBytes('annex-content-blind-v2'), welcome.subarray(0, 32), fromBase64(fixture.hello), welcome.subarray(32, 64));
  const host = new TunnelCipher(x25519.getSharedSecret(hexToBytes(fixture.serverSeed), fromBase64(fixture.hello).subarray(0, 32)), sha256(transcript), true);
  return { client, host };
}
describe('content-blind host tunnel', () => {
  it('matches independent OpenSSL handshake and directional encryption vectors', () => {
    const hello = clientHello(hexToBytes(fixture.clientSeed), hexToBytes(fixture.nonce));
    expect(hello.hello).toBe(fixture.hello);
    const client = hello.finish(fixture.host, fixture.welcome);
    expect(client.seal(fixture.request)).toEqual([fixture.requestPacket]);
    expect(client.open(fixture.responsePacket)).toEqual(fixture.response);
    const { host } = pair();
    expect(host.open(fixture.requestPacket)).toEqual(fixture.request);
    expect(host.seal(fixture.response)).toEqual([fixture.responsePacket]);
  });
  it('rejects host substitution, altered signatures, and replayed handshakes from another client', () => {
    const make = () => clientHello(hexToBytes(fixture.clientSeed), hexToBytes(fixture.nonce));
    expect(() => make().finish('0'.repeat(64), fixture.welcome)).toThrow('different server identity');
    const altered = fromBase64(fixture.welcome); altered[127] ^= 1;
    expect(() => make().finish(fixture.host, toBase64(altered))).toThrow('authenticate');
    expect(() => clientHello().finish(fixture.host, fixture.welcome)).toThrow('authenticate');
    const hello = make(); hello.finish(fixture.host, fixture.welcome);
    expect(() => hello.finish(fixture.host, fixture.welcome)).toThrow('consumed');
  });
  it('rejects ciphertext tampering, reflection, reordering, and duplicate actions', () => {
    const { host } = pair(); host.open(fixture.requestPacket);
    expect(() => host.open(fixture.requestPacket)).toThrow('replay');
    expect(() => pair().host.open({ ...fixture.requestPacket, sequence: 1 })).toThrow('missing packet');
    expect(() => pair().client.open(fixture.requestPacket)).toThrow();
    const bytes = fromBase64(fixture.requestPacket.data); bytes[12] ^= 1;
    expect(() => pair().host.open({ sequence: 0, data: toBase64(bytes) })).toThrow();
  });
  it('fragments a real large application frame and keeps it opaque on the wire', () => {
    const { client, host } = pair();
    const request = { type: 'http_data', content: 'private file bytes '.repeat(60_000) };
    const packets = client.seal(request); expect(packets.length).toBeGreaterThan(10);
    for (const packet of packets.slice(0, -1)) expect(host.open(packet)).toBeUndefined();
    expect(host.open(packets.at(-1)!)).toEqual(request);
    expect(JSON.stringify(packets)).not.toContain('private file');
    expect(JSON.stringify(packets)).not.toContain('http_data');
    expect(packets.every(packet => packet.data.length < 100_000)).toBe(true);
  });
  it('rejects a signed low-order key and erases the ephemeral secret on failure', () => {
    const secret = hexToBytes(fixture.clientSeed), hello = clientHello(secret, hexToBytes(fixture.nonce));
    const identity = ed25519.getPublicKey(hexToBytes(fixture.identitySeed));
    const text = concatBytes(utf8ToBytes('annex-content-blind-v2'), identity, fromBase64(hello.hello), new Uint8Array(32));
    const welcome = concatBytes(identity, new Uint8Array(32), ed25519.sign(text, hexToBytes(fixture.identitySeed)));
    expect(() => hello.finish(fixture.host, toBase64(welcome))).toThrow();
    expect(secret.every(byte => byte === 0)).toBe(true);
  });
});
