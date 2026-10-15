import { afterEach, describe, expect, it, vi } from 'vitest';
import { publicKeyFromSecret, toBase64, toHex } from '@/lib/e2e';
import { deviceIdForPublicKey } from '@/lib/e2e-v2';
import { parseChannelKeyDirectory, parseChannelKeyState, parseOwnChannelWraps } from './e2e-v2';

const state = () => ({ version: 2, server_identity: `annex-server:${'a'.repeat(64)}`,
  authority_identity: `annex-server:${'a'.repeat(64)}`,
  authority_server: 'https://annex.example', revision: 3, current_epoch: 1, required_epoch: 1,
  current_key_id: 'b'.repeat(64), history_policy: 'all_members', e2e_enabled: true,
  has_key: true, rotation_required: false, legacy_history_available: false, max_epoch: 1 });
const pub = publicKeyFromSecret(new Uint8Array(32).fill(9));
const member = () => ({ pseudonym_id: 'alice', device_id: deviceIdForPublicKey(pub),
  x25519_pub_hex: toHex(pub), device_certificate: null, certificate_sha256: 'c'.repeat(64), has_current_wrap: false });
const directory = () => ({ ...state(), member_keys: [member()], missing_current_devices: [member()],
  manifest: { head_seq: 0, head_hash: '', genesis_hash: null, join_policy: 'eligible_self_join', history_policy: 'all_members',
    e2e_enabled: true, members: [], revoked_devices: [] },
  historical_epochs: [], next_history_after_epoch: null, legacy_candidates: [], next_legacy_cursor: null });

afterEach(() => { vi.unstubAllGlobals(); vi.resetModules(); });

describe('v2 key-service response validation', () => {
  it('accepts the current state and complete missing-device snapshot', () => {
    expect(parseChannelKeyState(state())).toEqual(state());
    expect(parseChannelKeyDirectory(directory())).toEqual(directory());
  });
  it.each([
    { version: 1 }, { revision: 1.5 }, { required_epoch: 0 }, { current_epoch: Number.MAX_SAFE_INTEGER + 1 },
    { current_key_id: null }, { has_key: false }, { rotation_required: true }, { max_epoch: 2 },
    { server_identity: 'https://annex.example' }, { history_policy: 'anything' },
  ])('rejects incoherent or unsupported key state %j', invalid => {
    expect(() => parseChannelKeyState({ ...state(), ...invalid })).toThrow();
  });
  it('rejects a directory key whose advertised device ID does not match the public key', () => {
    const value = directory(); value.member_keys[0].device_id = 'f'.repeat(64);
    expect(() => parseChannelKeyDirectory(value)).toThrow('device');
  });
  it('rejects duplicate, omitted and already-wrapped tuples from the missing snapshot', () => {
    expect(() => parseChannelKeyDirectory({ ...directory(), member_keys: [member(), member()] })).toThrow('Duplicate');
    expect(() => parseChannelKeyDirectory({ ...directory(), missing_current_devices: [] })).toThrow('Incomplete');
    expect(() => parseChannelKeyDirectory({ ...directory(), missing_current_devices: [{ ...member(), has_current_wrap: true }] })).toThrow('snapshot');
  });
  it('rejects noncanonical or truncated sealed-wrap bytes before cryptographic processing', () => {
    const payload = { state: state(), wraps: [{ epoch: 1, key_id: 'a'.repeat(64), recipient_device_id: member().device_id,
      authority_namespace: state().authority_identity, authorization: null,
      sender_pseudonym_id: 'bob', sender_device_id: 'b'.repeat(64), wrapped_key_b64: toBase64(new Uint8Array(59)) }],
      next_after_epoch: null, legacy_device_wraps: [], next_legacy_cursor: null,
      legacy_wraps: [], next_legacy_wrap_after_epoch: null };
    expect(() => parseOwnChannelWraps(payload)).toThrow('Truncated');
    payload.wraps[0].wrapped_key_b64 = ` ${toBase64(new Uint8Array(60))}`;
    expect(() => parseOwnChannelWraps(payload)).toThrow('canonical');
  });
});

describe('bound encryption credential context', () => {
  it('rejects a late response after switching servers and does not make a second request to the new server', async () => {
    const core = await import('./core');
    const { boundE2eApiV2 } = await import('./e2e-v2');
    let release!: (value: Response) => void;
    const fetch = vi.fn((url: string) => String(url).endsWith('/key-status')
      ? new Promise<Response>(resolve => { release = resolve; })
      : Promise.reject(new Error('No upload grant required by this test')));
    vi.stubGlobal('fetch', fetch);
    core.setApiBaseUrl('https://a.example'); core.setSessionToken('token-a', 'alice');
    const api = boundE2eApiV2('alice');
    const pending = api.getState('channel');
    core.setApiBaseUrl('https://b.example'); core.setSessionToken('token-b', 'bob');
    release(new Response(JSON.stringify(state()), { status: 200 }));
    await expect(pending).rejects.toThrow('no longer current');
    await expect(api.getState('channel')).rejects.toThrow('no longer current');
    expect(fetch.mock.calls.filter(([url]) => String(url).endsWith('/key-status')).map(([url]) => url))
      .toEqual(['https://a.example/api/channels/channel/key-status']);
  });
});
