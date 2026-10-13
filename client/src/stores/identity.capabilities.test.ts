import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useIdentityStore } from './identity';
import { useVoiceStore } from './voice';
import { setApiBaseUrl, setSessionToken } from '@/api/core';
import type { IdentityInfo, StoredIdentity } from '@/types';

const state = vi.hoisted(() => ({ identity: vi.fn() }));
vi.mock('@/lib/api', () => ({ getIdentityInfo: (...args: unknown[]) => state.identity(...args), ApiError: class extends Error {} }));
vi.mock('@/lib/db', () => ({}));
vi.mock('@/lib/zk', () => ({}));
const person: StoredIdentity = { id: 'local', pseudonymId: 'alice', sk: '01', commitmentHex: '02', roleCode: 0,
  nodeId: 1, serverSlug: 'server', sessionToken: 'token', leafIndex: 0, zkProofPayload: null, createdAt: '' };
const info = (canVoice: boolean): IdentityInfo => ({ pseudonymId: 'alice', participantType: 'HUMAN', active: true,
  capabilities: { can_voice: canVoice, can_invite: true, can_bridge: false, can_federate: false, can_moderate: false } });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { resolve, promise }; }
beforeEach(() => {
  vi.clearAllMocks(); setApiBaseUrl(''); setSessionToken('token', 'alice');
  useVoiceStore.getState().forceReset();
  useIdentityStore.setState({ identity: person, permissions: info(true), permissionsStatus: 'ready', permissionsPseudonymId: 'alice' });
});

describe('live capability hints and authenticated permission snapshots', () => {
  it('ends voice synchronously even when the confirming identity request is still pending', async () => {
    const gate = deferred<IdentityInfo>(); state.identity.mockReturnValue(gate.promise);
    useVoiceStore.setState({ voiceToken: 'active', connectedChannelId: 'room', connectionState: 'connected', joiningAnyCall: true });
    const pending = useIdentityStore.getState().receiveCapabilitiesHint(false);
    expect(useVoiceStore.getState()).toMatchObject({ voiceToken: null, connectedChannelId: null, joiningAnyCall: false });
    expect(useIdentityStore.getState().permissionsStatus).toBe('loading');
    gate.resolve(info(false)); await pending;
    expect(useIdentityStore.getState().permissions?.capabilities.can_voice).toBe(false);
    expect(useIdentityStore.getState().permissionsStatus).toBe('ready');
  });
  it('does not allow an earlier granted HTTP response to overwrite a later revocation', async () => {
    const old = deferred<IdentityInfo>(), fresh = deferred<IdentityInfo>();
    state.identity.mockReturnValueOnce(old.promise).mockReturnValueOnce(fresh.promise);
    const first = useIdentityStore.getState().loadPermissions();
    const revoked = useIdentityStore.getState().receiveCapabilitiesHint(false);
    old.resolve(info(true)); await first;
    expect(useIdentityStore.getState().permissionsStatus).toBe('loading');
    expect(useVoiceStore.getState().voiceToken).toBeNull();
    fresh.resolve(info(false)); await revoked;
    expect(useIdentityStore.getState().permissions?.capabilities.can_voice).toBe(false);
  });
  it('requires authenticated confirmation to restore Join and never reopens the old call', async () => {
    state.identity.mockResolvedValueOnce(info(false)); await useIdentityStore.getState().receiveCapabilitiesHint(false);
    const gate = deferred<IdentityInfo>(); state.identity.mockReturnValueOnce(gate.promise);
    const restoring = useIdentityStore.getState().receiveCapabilitiesHint(true);
    expect(useIdentityStore.getState().permissionsStatus).toBe('loading');
    gate.resolve(info(true)); await restoring;
    expect(useIdentityStore.getState().permissions?.capabilities.can_voice).toBe(true);
    expect(useVoiceStore.getState()).toMatchObject({ voiceToken: null, connectedChannelId: null, joiningAnyCall: false });
  });
  it('ignores a stale permission reply after the server credential context changes', async () => {
    const gate = deferred<IdentityInfo>(); state.identity.mockReturnValueOnce(gate.promise);
    const pending = useIdentityStore.getState().loadPermissions();
    setApiBaseUrl('https://different-server.example');
    useIdentityStore.setState({ permissions: info(false), permissionsStatus: 'ready' });
    gate.resolve(info(true)); await pending;
    expect(useIdentityStore.getState().permissions?.capabilities.can_voice).toBe(false);
  });
  it('an allow hint whose HTTP confirmation still denies voice keeps Join blocked', async () => {
    state.identity.mockResolvedValue(info(false));
    await useIdentityStore.getState().receiveCapabilitiesHint(false);
    await useIdentityStore.getState().receiveCapabilitiesHint(true);
    expect(useIdentityStore.getState().permissionsStatus).toBe('ready');
    expect(useIdentityStore.getState().permissions?.capabilities.can_voice).toBe(false);
  });
  it('a prior-context restore action cannot clear the current context voice refusal', async () => {
    const gate = deferred<IdentityInfo>(); state.identity.mockReturnValueOnce(gate.promise);
    const pending = useIdentityStore.getState().receiveCapabilitiesHint(true);
    setApiBaseUrl('https://different-server.example');
    useIdentityStore.setState({ permissions: info(true), permissionsStatus: 'ready' });
    useVoiceStore.getState().setVoiceSessionDisabled(true, 'Current context is blocked');
    gate.resolve(info(true)); await pending;
    expect(useVoiceStore.getState().voiceSessionDisabled).toBe(true);
  });
  it('the ordinary Retry read recovers a failed restore without rejoining', async () => {
    state.identity.mockResolvedValueOnce(info(false)); await useIdentityStore.getState().receiveCapabilitiesHint(false);
    state.identity.mockRejectedValueOnce(new Error('temporary network failure'));
    await useIdentityStore.getState().receiveCapabilitiesHint(true);
    expect(useIdentityStore.getState().permissionsStatus).toBe('error');
    state.identity.mockResolvedValueOnce(info(true)); await useIdentityStore.getState().loadPermissions();
    expect(useIdentityStore.getState().permissionsStatus).toBe('ready');
    expect(useIdentityStore.getState().permissions?.capabilities.can_voice).toBe(true);
    expect(useVoiceStore.getState()).toMatchObject({ voiceToken: null, connectedChannelId: null, joiningAnyCall: false });
  });
});
