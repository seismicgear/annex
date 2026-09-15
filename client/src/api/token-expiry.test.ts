/**
 * `isTokenExpired` must understand the token the server actually mints.
 *
 * Session tokens gained a `token_epoch` field when per-identity revocation
 * landed, moving from `pseudonym|expires|sig` to `pseudonym|epoch|expires|sig`.
 * This helper still counted three fields and returned `true` for anything
 * else — so every v2 token read as already expired and the client refreshed
 * on every check.
 *
 * The failure mode is quiet in both directions, which is why it needs pinning:
 * an always-expired token produces a refresh storm rather than an error, and
 * reading the epoch as a timestamp would produce "expired in 1970" for every
 * never-revoked identity.
 */
import { describe, it, expect } from 'vitest';
import { isTokenExpired } from './core';

/** Encode a token the way the server does: base64 of the pipe-joined fields. */
function encode(fields: (string | number)[]): string {
  return btoa(fields.join('|'));
}

const FUTURE = Math.floor(Date.now() / 1000) + 3600;
const PAST = Math.floor(Date.now() / 1000) - 3600;

describe('isTokenExpired', () => {
  it('reads the expiry from a v2 token (pseudonym|epoch|expires|sig)', () => {
    expect(isTokenExpired(encode(['alice', 0, FUTURE, 'sig']))).toBe(false);
    expect(isTokenExpired(encode(['alice', 0, PAST, 'sig']))).toBe(true);
  });

  it('is not fooled by a non-zero epoch', () => {
    // A revoked-and-reissued identity has a large epoch. Reading field 1 as the
    // expiry would make this look valid until the year the epoch resembles.
    expect(isTokenExpired(encode(['alice', 7, PAST, 'sig']))).toBe(true);
    expect(isTokenExpired(encode(['alice', 7, FUTURE, 'sig']))).toBe(false);
  });

  it('still understands a v1 token', () => {
    // Older clients and stored sessions predate the epoch field.
    expect(isTokenExpired(encode(['alice', FUTURE, 'sig']))).toBe(false);
    expect(isTokenExpired(encode(['alice', PAST, 'sig']))).toBe(true);
  });

  it('treats anything it cannot parse as expired', () => {
    expect(isTokenExpired('not-base64-at-all!!')).toBe(true);
    expect(isTokenExpired(encode(['alice']))).toBe(true);
    expect(isTokenExpired(encode(['a', 'b', 'c', 'd', 'e']))).toBe(true);
    expect(isTokenExpired(encode(['alice', 0, 'not-a-number', 'sig']))).toBe(true);
  });
});
