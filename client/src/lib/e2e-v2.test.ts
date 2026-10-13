import { describe, expect, it } from 'vitest';
import { sha256 } from '@noble/hashes/sha2.js';
import { encryptContent, generateChannelKey, generateDeviceSecret, publicKeyFromSecret, sealTo, toBase64, toHex, utf8 } from './e2e';
import {
  attachmentAad, channelKeyId, decodeCanonicalBase64, decodeHex32, decryptAttachment,
  decryptMessageV2, deviceIdForPublicKey, encryptAttachment, encryptMessageV2, messageAadV2,
  openChannelKeyV2, parseMessageV2, sealChannelKeyV2, type EpochContext, type WrapContext,
} from './e2e-v2';

const cek = new Uint8Array(32).fill(0x17);
const secret = new Uint8Array(32).fill(0x42);
const pub = publicKeyFromSecret(secret);
const context: EpochContext = { authorityServer: 'https://annex.example', channelId: 'room', epoch: 7, keyId: channelKeyId(cek) };
const wrapContext: WrapContext = { ...context, recipientPseudonym: 'member', recipientDeviceId: deviceIdForPublicKey(pub) };

describe('strict v2 encodings', () => {
  it.each(['0x' + 'ff'.repeat(32), 'GG'.repeat(32), 'FF'.repeat(32), 'f'.repeat(63), '  ' + 'ff'.repeat(31)])('rejects noncanonical hex %s', value => {
    expect(() => decodeHex32(value)).toThrow();
  });
  it.each(['Zg', 'Zh==', ' Zg==', 'Zg==\n', 'Zg===', 'Zg-_', '===='])('rejects noncanonical base64 %s', value => {
    expect(() => decodeCanonicalBase64(value)).toThrow();
  });
  it('accepts canonical bytes and enforces the decoded size bound', () => {
    expect(decodeCanonicalBase64('Zg==', 1)).toEqual(new Uint8Array([102]));
    expect(() => decodeCanonicalBase64('Zm8=', 1)).toThrow();
    expect(decodeHex32('ab'.repeat(32))).toEqual(new Uint8Array(32).fill(0xab));
  });
  it('derives domain-separated key IDs and rejects low-order device keys', () => {
    const bytes = new Uint8Array(utf8.encode('annex-channel-key-v2\0').length + 32);
    bytes.set(utf8.encode('annex-channel-key-v2\0'));
    bytes.set(cek, bytes.length - 32);
    expect(context.keyId).toBe(toHex(sha256(bytes)));
    expect(context.keyId).not.toBe(toHex(sha256(cek)));
    expect(() => deviceIdForPublicKey(new Uint8Array(32))).toThrow();
  });
});

describe('context-authenticated device wrapping', () => {
  it('opens only the intended channel, epoch, identity and device', () => {
    const sealed = sealChannelKeyV2(cek, pub, wrapContext);
    expect(openChannelKeyV2(sealed, secret, wrapContext)).toEqual(cek);
    for (const altered of [
      { authorityServer: 'https://other.example' }, { channelId: 'another-room' },
      { epoch: 8 }, { recipientPseudonym: 'outsider' }, { keyId: 'ab'.repeat(32) },
    ]) expect(() => openChannelKeyV2(sealed, secret, { ...wrapContext, ...altered })).toThrow();
    expect(() => openChannelKeyV2(sealed, generateDeviceSecret(), wrapContext)).toThrow();
  });
  it('rejects raw legacy keys, noncanonical envelopes and false key fingerprints', () => {
    expect(() => openChannelKeyV2(toBase64(sealTo(cek, pub)), secret, wrapContext)).toThrow();
    const fields = ['annex-key-wrap-v2', context.authorityServer, context.channelId, context.epoch,
      context.keyId, wrapContext.recipientPseudonym, wrapContext.recipientDeviceId, toBase64(cek)];
    expect(() => openChannelKeyV2(toBase64(sealTo(utf8.encode(JSON.stringify(fields, null, 2)), pub)), secret, wrapContext)).toThrow();
    expect(() => sealChannelKeyV2(generateChannelKey(), pub, wrapContext)).toThrow();
    expect(() => sealChannelKeyV2(cek, publicKeyFromSecret(generateDeviceSecret()), wrapContext)).toThrow();
  });
  it('keeps legacy bridge candidates cryptographically separate from committed epochs', () => {
    const legacy = sealChannelKeyV2(cek, pub, wrapContext, 'legacy');
    expect(openChannelKeyV2(legacy, secret, wrapContext, 'legacy')).toEqual(cek);
    expect(() => openChannelKeyV2(legacy, secret, wrapContext)).toThrow();
    const current = sealChannelKeyV2(cek, pub, wrapContext);
    expect(() => openChannelKeyV2(current, secret, wrapContext, 'legacy')).toThrow();
  });
});

describe('epoch-addressed messages', () => {
  it('round-trips actual AEAD and authenticates the authority, channel, epoch and key ID', () => {
    const body = encryptMessageV2('private message 🔒', cek, context);
    expect(body).not.toContain('private message');
    expect(parseMessageV2(body).epoch).toBe(7);
    expect(decryptMessageV2(body, cek, context)).toBe('private message 🔒');
    expect(() => decryptMessageV2(body, cek, { ...context, authorityServer: 'https://other.example' })).toThrow();
    expect(() => decryptMessageV2(body, cek, { ...context, channelId: 'other' })).toThrow();
    expect(() => decryptMessageV2(body.replace('e2e2:7:', 'e2e2:8:'), cek, { ...context, epoch: 8 })).toThrow();
    const other = generateChannelKey();
    expect(() => decryptMessageV2(body, other, { ...context, keyId: channelKeyId(other) })).toThrow();
  });
  it.each(['0', '01', '-1', '1.2', '1e2', '9007199254740992'])('rejects invalid epoch %s', epoch => {
    expect(() => parseMessageV2(`e2e2:${epoch}:${context.keyId}:${toBase64(new Uint8Array(28))}`)).toThrow();
  });
  it('rejects truncated payloads, tampering and authenticated invalid UTF-8', () => {
    expect(() => parseMessageV2(`e2e2:7:${context.keyId}:AA==`)).toThrow();
    const invalid = encryptContent(cek, new Uint8Array([0xc3, 0x28]), messageAadV2(context));
    expect(() => decryptMessageV2(`e2e2:7:${context.keyId}:${toBase64(invalid)}`, cek, context)).toThrow('UTF-8');
    const body = encryptMessageV2('untampered', cek, context);
    const parsed = parseMessageV2(body);
    parsed.ciphertext[parsed.ciphertext.length - 1] ^= 1;
    expect(() => decryptMessageV2(`e2e2:7:${context.keyId}:${toBase64(parsed.ciphertext)}`, cek, context)).toThrow();
    expect(() => encryptMessageV2('\ud800', cek, context)).toThrow('Unicode');
  });
});

describe('opaque attachments', () => {
  const attachmentId = '48092941-a613-4b7f-9654-2823e416f74c';
  it('authenticates raw bytes, descriptor size, ciphertext digest and attachment context', () => {
    const bytes = new Uint8Array([0, 255, 34, 19, 0]);
    const key = generateChannelKey();
    const encrypted = encryptAttachment(bytes, key, context, attachmentId);
    const hash = toHex(sha256(encrypted));
    expect(encrypted.length).toBe(bytes.length + 28);
    expect(decryptAttachment(encrypted, key, context, attachmentId, hash, bytes.length)).toEqual(bytes);
    expect(() => decryptAttachment(encrypted, key, context, attachmentId, hash, 6)).toThrow();
    expect(() => decryptAttachment(encrypted, key, context, attachmentId, '00'.repeat(32), bytes.length)).toThrow();
    expect(() => decryptAttachment(encrypted, key, { ...context, epoch: 8 }, attachmentId, hash, bytes.length)).toThrow();
    expect(() => decryptAttachment(encrypted, key, context, attachmentId.replace('48092941', '48092942'), hash, bytes.length)).toThrow();
    expect(() => attachmentAad(context, attachmentId.toUpperCase())).toThrow();
  });
});
