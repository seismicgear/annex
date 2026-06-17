import { describe, it, expect } from 'vitest';
import {
  sealTo,
  openFrom,
  publicKeyFromSecret,
  generateChannelKey,
  generateDeviceSecret,
  encryptContent,
  decryptContent,
  toBase64,
  fromBase64,
  fromHex,
  toHex,
  utf8,
  __test__,
} from './e2e';

// These three vectors are FROZEN and identical to the Rust KAT
// (annex-federation `seal::tests::x25519_kat_is_stable_and_cross_language`).
// If the construction drifts in either language this test fails — that is the
// guarantee that a key wrapped by the Rust server opens in this browser client.
const KAT_RECIPIENT_SECRET = fromHex(
  '0102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f20',
);
const KAT_EPHEMERAL_SECRET = fromHex(
  'a0a1a2a3a4a5a6a7a8a9aaabacadaeafb0b1b2b3b4b5b6b7b8b9babbbcbdbebf',
);
const KAT_NONCE = fromHex('000102030405060708090a0b');
const KAT_PLAINTEXT = utf8.encode('annex-e2e-kat-v1 channel content key payload');
const KAT_EXPECTED_RECIPIENT_PUB =
  '07a37cbc142093c8b755dc1b10e86cb426374ad16aa853ed0bdfc0b2b86d1c7c';
const KAT_EXPECTED_WIRE =
  '605a725d2a4adfeeb1a29e17edd621c1b7593ee8cdbc44ac6c4ab6e2f805d23c' +
  '000102030405060708090a0b' +
  'c70438103cf37965facd5e288820f2e8ee205588a4da314bb857d2ed407e95f8' +
  'abd7be0b6bec226711e8ba00657a946ad787ac6c6af1877c65cd6f3b';

describe('e2e sealed box — cross-language KAT', () => {
  it('derives the same recipient public key as Rust', () => {
    expect(toHex(publicKeyFromSecret(KAT_RECIPIENT_SECRET))).toBe(KAT_EXPECTED_RECIPIENT_PUB);
  });

  it('produces byte-identical wire bytes to the Rust seal', () => {
    const recipientPub = publicKeyFromSecret(KAT_RECIPIENT_SECRET);
    const wire = __test__.sealToWith(KAT_PLAINTEXT, recipientPub, KAT_EPHEMERAL_SECRET, KAT_NONCE);
    expect(toHex(wire)).toBe(KAT_EXPECTED_WIRE);
  });

  it('opens the frozen Rust-produced wire', () => {
    const opened = openFrom(fromHex(KAT_EXPECTED_WIRE), KAT_RECIPIENT_SECRET);
    expect(utf8.decode(opened)).toBe('annex-e2e-kat-v1 channel content key payload');
  });
});

describe('e2e sealed box — functional', () => {
  it('round-trips a wrapped channel key', () => {
    const deviceSecret = generateDeviceSecret();
    const devicePub = publicKeyFromSecret(deviceSecret);
    const cek = generateChannelKey();
    const wrapped = sealTo(cek, devicePub);
    expect(openFrom(wrapped, deviceSecret)).toEqual(cek);
  });

  it('does not leak the plaintext key into the wire', () => {
    const deviceSecret = generateDeviceSecret();
    const cek = generateChannelKey();
    const wrapped = sealTo(cek, publicKeyFromSecret(deviceSecret));
    // The raw CEK bytes must not appear in the sealed output.
    const hay = toHex(wrapped);
    expect(hay.includes(toHex(cek))).toBe(false);
  });

  it('a non-recipient cannot open it', () => {
    const recipient = generateDeviceSecret();
    const attacker = generateDeviceSecret();
    const wrapped = sealTo(generateChannelKey(), publicKeyFromSecret(recipient));
    expect(() => openFrom(wrapped, attacker)).toThrow();
  });

  it('detects tampering', () => {
    const recipient = generateDeviceSecret();
    const wrapped = sealTo(generateChannelKey(), publicKeyFromSecret(recipient));
    wrapped[wrapped.length - 1] ^= 0x01;
    expect(() => openFrom(wrapped, recipient)).toThrow();
  });

  it('each seal is unique (fresh ephemeral + nonce)', () => {
    const recipient = publicKeyFromSecret(generateDeviceSecret());
    const cek = generateChannelKey();
    expect(toHex(sealTo(cek, recipient))).not.toBe(toHex(sealTo(cek, recipient)));
  });
});

describe('e2e channel content encryption', () => {
  it('round-trips a message body', () => {
    const cek = generateChannelKey();
    const body = utf8.encode('hello, this stays server-blind 🔒');
    const blob = encryptContent(cek, body);
    expect(utf8.decode(decryptContent(cek, blob))).toBe('hello, this stays server-blind 🔒');
  });

  it('ciphertext does not contain the plaintext', () => {
    const cek = generateChannelKey();
    const blob = encryptContent(cek, utf8.encode('SECRET-MARKER-1234'));
    expect(utf8.decode(blob).includes('SECRET-MARKER-1234')).toBe(false);
  });

  it('binds AAD: ciphertext from one context fails to open under another', () => {
    const cek = generateChannelKey();
    const blob = encryptContent(cek, utf8.encode('x'), utf8.encode('channel:a'));
    expect(() => decryptContent(cek, blob, utf8.encode('channel:b'))).toThrow();
    expect(utf8.decode(decryptContent(cek, blob, utf8.encode('channel:a')))).toBe('x');
  });

  it('wrong key fails', () => {
    const blob = encryptContent(generateChannelKey(), utf8.encode('x'));
    expect(() => decryptContent(generateChannelKey(), blob)).toThrow();
  });

  it('base64 helpers round-trip', () => {
    const b = generateChannelKey();
    expect(fromBase64(toBase64(b))).toEqual(b);
  });
});
