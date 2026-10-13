import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { listProfileMetadataGrants, setProfileMetadataGrant } from './profile-metadata';
import { setSessionToken, setApiBaseUrl } from './core';

const snapshot = { owner_pseudonym: 'owner', metadata_json: null, grantees: [] };
const json = (value: unknown) => new Response(JSON.stringify(value), { status: 200 });
beforeEach(() => { setApiBaseUrl(''); setSessionToken('session', 'owner'); vi.stubGlobal('fetch', vi.fn()); });
afterEach(() => { setSessionToken(null, null); vi.unstubAllGlobals(); });

describe('owner-scoped profile metadata grants', () => {
  it('uses authenticated owner inference and sends no owner or metadata on a grant/revoke', async () => {
    vi.mocked(fetch).mockResolvedValueOnce(json(snapshot)).mockResolvedValueOnce(json({ granted: true })).mockResolvedValueOnce(json({ granted: false }));
    expect(await listProfileMetadataGrants()).toEqual(snapshot);
    await setProfileMetadataGrant('reader/a', true);
    await setProfileMetadataGrant('reader/a', false);
    expect(vi.mocked(fetch).mock.calls.map(([url]) => String(url))).toEqual([
      '/api/graph/metadata-grants', '/api/graph/metadata-grants/reader%2Fa', '/api/graph/metadata-grants/reader%2Fa',
    ]);
    const changes = vi.mocked(fetch).mock.calls.slice(1).map(([, init]) => init!);
    expect(changes.map(init => init.method)).toEqual(['PUT', 'DELETE']);
    expect(changes.every(init => init.body === undefined)).toBe(true);
    expect(changes.every(init => new Headers(init.headers).get('Authorization') === 'Bearer session')).toBe(true);
  });

  it.each([
    { ...snapshot, owner_pseudonym: 'someone-else' },
    { ...snapshot, metadata_json: '{broken' },
    { ...snapshot, metadata_json: undefined },
    { ...snapshot, grantees: ['reader', 'reader'] },
    { ...snapshot, grantees: ['owner'] },
  ])('refuses a malformed or wrong-owner snapshot %#', async value => {
    vi.mocked(fetch).mockResolvedValue(json(value));
    await expect(listProfileMetadataGrants()).rejects.toThrow();
  });

  it('does not apply a delayed privacy response to another signed-in identity', async () => {
    let finish!: (response: Response) => void;
    vi.mocked(fetch).mockReturnValue(new Promise(resolve => { finish = resolve; }));
    const pending = listProfileMetadataGrants();
    setSessionToken('different', 'other');
    finish(json(snapshot));
    await expect(pending).rejects.toThrow('context');
  });

  it('refuses a misleading acknowledgement and unauthenticated/self grants', async () => {
    vi.mocked(fetch).mockResolvedValue(json({ granted: false }));
    await expect(setProfileMetadataGrant('reader', true)).rejects.toThrow('did not confirm');
    await expect(setProfileMetadataGrant('owner', true)).rejects.toThrow('another participant');
    setSessionToken(null, null);
    await expect(listProfileMetadataGrants()).rejects.toThrow('Sign in');
    expect(fetch).toHaveBeenCalledTimes(1);
  });
});
