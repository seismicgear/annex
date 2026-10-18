import { describe, it, expect } from 'vitest';
import { canCreateInviteLink, parseInviteLink } from './invite';

describe('pasted invitation handoff', () => {
  const server = `https://www.monolithannex.com/host/${'a'.repeat(64)}`;
  const link = (value: unknown) => `https://monolithannex.com/invite/${btoa(JSON.stringify(value)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')}`;
  it('preserves the exact server and admission code from a share link or protocol link', () => {
    const payload = { server, code: 'one-time-admission', serverName: 'My Server' };
    expect(parseInviteLink(link(payload))).toEqual({ server, code: payload.code });
    expect(parseInviteLink(`annex://invite?server=${encodeURIComponent(server)}&code=${payload.code}`)).toEqual({ server, code: payload.code });
    expect(parseInviteLink(server)).toBeNull();
    expect(parseInviteLink('127.0.0.1:3000')).toBeNull();
  });
  it.each([
    { server: 'https://trusted.example@wrong.example', code: 'code' },
    { server: 'http://plain.example', code: 'code' },
    { server: `${server}?redirect=elsewhere`, code: 'code' },
    { server, code: '' }, { server, code: 'a'.repeat(101) }, { server, code: 'line\nbreak' },
  ])('rejects malformed or credential-bearing invitations before contacting a destination', value => {
    expect(() => parseInviteLink(link(value))).toThrow(/invite link/);
  });
  it('refuses broken payloads and duplicate deep-link destinations', () => {
    expect(() => parseInviteLink('https://monolithannex.com/invite/broken')).toThrow(/invite link/);
    expect(() => parseInviteLink(`annex://invite?server=${encodeURIComponent(server)}&server=https://other.example&code=code`)).toThrow(/invite link/);
  });
});

/**
 * The invite format requires HTTPS, because the link carries a join secret.
 * Three call sites used to test only that a public URL was *set* — the admin
 * panel on open and after a save, and startup's pre-create — so every http://
 * deployment fired an invite request that always came back 400, and all three
 * swallowed it in an empty catch. The operator saw no invite link and no
 * reason for its absence. The UI audit caught the repeated 400 on
 * `admin-server-settings`.
 */
describe('canCreateInviteLink', () => {
  it('accepts an https URL', () => {
    expect(canCreateInviteLink('https://annex.example.com')).toBe(true);
  });

  it('rejects http, which the server will reject too', () => {
    expect(canCreateInviteLink('http://annex.example.com')).toBe(false);
    expect(canCreateInviteLink('http://127.0.0.1:3000')).toBe(false);
  });

  it('rejects an unset public URL', () => {
    expect(canCreateInviteLink('')).toBe(false);
    expect(canCreateInviteLink(null)).toBe(false);
    expect(canCreateInviteLink(undefined)).toBe(false);
  });

  it('ignores surrounding whitespace', () => {
    expect(canCreateInviteLink('  https://annex.example.com  ')).toBe(true);
    expect(canCreateInviteLink('   ')).toBe(false);
  });

  it('is case-insensitive about the scheme', () => {
    expect(canCreateInviteLink('HTTPS://annex.example.com')).toBe(true);
  });

  // A URL that merely mentions https must not pass — the scheme has to be the
  // scheme, or the check would wave through exactly what it exists to stop.
  it('requires https to be the scheme, not merely present', () => {
    expect(canCreateInviteLink('http://evil.example.com/?next=https://annex.example.com')).toBe(false);
    expect(canCreateInviteLink('annex://https://annex.example.com')).toBe(false);
    expect(canCreateInviteLink('httpsx://annex.example.com')).toBe(false);
  });
});
