import { afterEach, describe, expect, it, vi } from 'vitest';
import { redeemInvite } from './identity';

afterEach(() => vi.unstubAllGlobals());

describe('invite redemption destination', () => {
  it.each([
    ['https://invite.example', 'https://invite.example/api/invites/redeem'],
    ['https://invite.example/', 'https://invite.example/api/invites/redeem'],
    ['https://invite.example:8443/', 'https://invite.example:8443/api/invites/redeem'],
    ['https://[2001:db8::1]:8443/', 'https://[2001:db8::1]:8443/api/invites/redeem'],
  ])('joins the API path to the accepted origin %s without a double slash', async (origin, endpoint) => {
    const result = { valid: true, serverName: 'Invited server', serverSlug: 'invited' };
    const fetch = vi.fn().mockResolvedValue(new Response(JSON.stringify(result), { status: 200 }));
    vi.stubGlobal('fetch', fetch);

    expect(await redeemInvite(origin, 'invitation-fixture')).toEqual(result);
    expect(fetch).toHaveBeenCalledWith(endpoint, {
      method: 'POST', headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ code: 'invitation-fixture' }),
    });
  });

  it('retains an actual refusal from the normalized target instead of treating it as validation', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify({ error: 'Invite expired' }), { status: 400 })));
    await expect(redeemInvite('https://invite.example/', 'expired-fixture')).rejects.toThrow('Invite expired');
  });
});
