// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { preparePrivateAttachment, openPrivateAttachment, parsePrivateMessage, privateMessage, messageText, parsePrivateAttachment } from './encrypted-attachments';
import { channelKeyId } from './e2e-v2';
import { generateChannelKey } from './e2e';
const policy = { images_enabled: true, videos_enabled: true, files_enabled: true, max_image_size_mb: 5, max_video_size_mb: 10, max_file_size_mb: 5 };
const context = { authorityServer: 'annex-server:' + 'a'.repeat(64), channelId: 'channel', epoch: 7, keyId: channelKeyId(generateChannelKey()) };
const origin = { originIdentity: context.authorityServer, originatingServer: 'https://annex.example' };
const create = (bytes = new TextEncoder().encode('actual private file bytes')) => {
  const output = preparePrivateAttachment(bytes, 'original name.txt', 'text/plain', policy, context);
  return { ...output, descriptor: parsePrivateAttachment({ ...output.descriptor, ...origin }) };
};
describe('private encrypted attachments', () => {
  it('roundtrips random per-file AEAD without storing plaintext or filenames in bytes', () => {
    const first = create(), second = create();
    expect(first.descriptor.fileKeyHex).not.toBe(second.descriptor.fileKeyHex);
    expect(new TextDecoder().decode(first.ciphertext)).not.toContain('private file bytes');
    expect(new TextDecoder().decode(first.ciphertext)).not.toContain('original name');
    expect(new TextDecoder().decode(openPrivateAttachment(first.ciphertext, first.descriptor, 'channel'))).toBe('actual private file bytes');
    const body = privateMessage('caption', [first.descriptor]);
    expect(parsePrivateMessage(body)).toEqual({ text: 'caption', attachments: [first.descriptor] });
    expect(messageText(body)).toBe('caption');
  });
  it('authenticates bytes, plaintext hash, file key and full epoch/channel context', () => {
    const { ciphertext, descriptor } = create();
    const corrupt = ciphertext.slice(); corrupt[20] ^= 1;
    expect(() => openPrivateAttachment(corrupt, descriptor, 'channel')).toThrow();
    expect(() => openPrivateAttachment(ciphertext, descriptor, 'another')).toThrow();
    for (const patch of [{ plaintextSha256: '0'.repeat(64) }, { fileKeyHex: '0'.repeat(64) }, { epoch: 8 }, { authorityNamespace: 'https://other.example' }]) {
      expect(() => openPrivateAttachment(ciphertext, { ...descriptor, ...patch }, 'channel')).toThrow();
    }
  });
  it('does not fetch arbitrary origins or accept HTML/SVG execution descriptors', () => {
    const { descriptor } = create();
    for (const patch of [{ url: 'https://attacker.example/file' }, { url: '/uploads/chat/files/../../secret' }, { contentType: 'text/html' },
      { category: 'image', contentType: 'image/svg+xml' }, { name: '../secret.txt' }, { bytes: Number.MAX_SAFE_INTEGER }]) {
      expect(() => parsePrivateAttachment({ ...descriptor, ...patch })).toThrow();
    }
  });
  it('keeps ordinary JSON ordinary and suppresses malformed private metadata in previews', () => {
    expect(parsePrivateMessage('{"text":"ordinary"}')).toBeNull();
    expect(messageText('annex.message.v1:{"fileKeyHex":"secret"}')).toBe('Encrypted attachment could not be verified');
  });
  it('enforces advertised category/size policy and detects renamed executables before encryption', () => {
    expect(() => preparePrivateAttachment(new TextEncoder().encode('MZ executable'), 'x.txt', 'text/plain', policy, context)).toThrow(/Executable/);
    expect(() => preparePrivateAttachment(new TextEncoder().encode('hello'), 'x.txt', 'text/plain', { ...policy, files_enabled: false }, context)).toThrow(/disabled/);
    expect(() => preparePrivateAttachment(new Uint8Array(1024 * 1024 + 1), 'x.bin', 'application/octet-stream', { ...policy, max_file_size_mb: 1 }, context)).toThrow(/size/);
  });
  it.each([['progressive.jpg','image/jpeg'],['animated.png','image/png'],['animated.gif','image/gif'],['animated.webp','image/webp']])('strips metadata before %s is encrypted', (name, mime) => {
    const bytes = new Uint8Array(readFileSync(new URL(`../../../crates/annex-server/tests/fixtures/image-privacy/${name}`, import.meta.url)));
    const output = preparePrivateAttachment(bytes, name, mime, policy, context);
    const descriptor = parsePrivateAttachment({ ...output.descriptor, ...origin });
    const plaintext = openPrivateAttachment(output.ciphertext, descriptor, 'channel');
    expect(new TextDecoder().decode(plaintext)).not.toContain('ANNEX_PRIVATE_METADATA');
    expect(plaintext.length).toBeLessThan(bytes.length);
  });
});
