import { act, cleanup, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useState } from 'react';
import type { StoredIdentity } from '@/types';
import type { IdentityPhase } from '@/stores/identity';
import type { NativeInviteDelivery } from '@/lib/tauri';

const m = vi.hoisted(() => ({
  listen: vi.fn(), pending: vi.fn(), ack: vi.fn(), saveMode: vi.fn(), redeem: vi.fn(),
  remote: vi.fn(), cleanup: vi.fn(), setIdentity: vi.fn(),
  summary: vi.fn(), register: vi.fn(), generate: vi.fn(),
  identity: null as StoredIdentity | null, error: null as string | null,
  base: 'https://old.example', registrationError: null as string | null,
}));
vi.mock('@/lib/tauri', () => ({ listenForInvite: m.listen, getPendingInvite: m.pending,
  ackPendingInvite: m.ack, saveStartupMode: m.saveMode }));
vi.mock('@/lib/api', () => ({ redeemInvite: m.redeem, getApiBaseUrl: () => m.base,
  getServerSummary: m.summary, ApiError: class extends Error {},
}));
vi.mock('@/lib/invite', () => ({ parseLegacyInviteFromUrl: () => null, clearInviteFromUrl: vi.fn() }));
vi.mock('@/stores/identity', () => ({ useIdentityStore: {
  getState: () => ({ identity: m.identity, error: m.error, generateLocalKeys: m.generate }), setState: m.setIdentity,
} }));
vi.mock('@/stores/servers', () => ({ useServersStore: { getState: () => ({
  beginRemoteRegistration: m.remote, cleanupFailedRegistration: m.cleanup,
  registrationError: m.registrationError, pendingRegistrationServerId: 'remote',
}) } }));
import { useInviteFlow } from './useInviteFlow';
import { useServerSelection } from './useServerSelection';

const local = { id: 'local', sk: '1234', commitmentHex: '5678', roleCode: 1, nodeId: 1,
  createdAt: '', serverSlug: '', pseudonymId: null, sessionToken: null, leafIndex: null } satisfies StoredIdentity;
const delivery: NativeInviteDelivery = { server: 'https://invited.example', code: 'genuine-boundary-fixture', deliveryId: 'delivery-a' };
function deferred<T>() {
  let resolve!: (value: T) => void; let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
let notify: (value: NativeInviteDelivery) => void;
let flow: { current: number };
const actions = { joinChannel: vi.fn(), loadChannels: vi.fn(), selectChannel: vi.fn(), setActiveView: vi.fn() };
function setup(initial: { phase: IdentityPhase; identity: StoredIdentity | null; ready: boolean } = {
  phase: 'keys_ready', identity: local, ready: false,
}, registerAutomatically = false) {
  m.identity = initial.identity;
  return renderHook(({ phase, identity }) => {
    const [serverReady, setServerReady] = useState(initial.ready);
    const [code, setCode] = useState<string | null>(null);
    const [slug, setSlug] = useState<string | null>(null);
    const invite = useInviteFlow({ phase, identity, serverReady, inTauri: true,
      ...actions, startupFlowId: flow, beginStartupFlow: () => ++flow.current,
      setServerReady, setPendingInviteCode: setCode, setPendingServerSlug: setSlug,
    });
    useServerSelection({ phase, identity, serverReady: registerAutomatically && serverReady, inTauri: false,
      registerWithServer: m.register, saveCurrentServer: vi.fn(), pendingInviteCode: code, pendingServerSlug: slug,
      serverPassword: '', startupFlowId: flow, setServerReady, setPendingInviteCode: setCode,
      setPendingServerSlug: setSlug, setPasswordRequired: vi.fn(), setServerPassword: vi.fn(),
      setStartupErrorDetails: vi.fn(), setDegradedStartup: vi.fn(),
    });
    return { ...invite, serverReady, code, slug };
  }, { initialProps: { phase: initial.phase, identity: initial.identity } });
}
async function deliver(value = delivery) {
  m.pending.mockResolvedValue(value);
  await act(async () => { notify(value); });
}
beforeEach(() => {
  vi.resetAllMocks(); flow = { current: 0 }; m.base = 'https://old.example'; m.error = null; m.registrationError = null;
  m.listen.mockImplementation(async (callback: typeof notify) => { notify = callback; return vi.fn(); });
  m.pending.mockResolvedValue(null); m.ack.mockResolvedValue(undefined); m.saveMode.mockResolvedValue(undefined);
  m.summary.mockResolvedValue({ slug: 'default-slug', access_mode: 'invite_only' }); m.register.mockResolvedValue(undefined);
  m.redeem.mockResolvedValue({ valid: true, serverSlug: 'destination', serverName: 'Destination' });
  m.remote.mockImplementation(async () => {
    m.base = delivery.server; m.identity = { ...local, id: 'remote-identity' }; return { id: 'remote' };
  });
});
afterEach(cleanup);

describe('native invite lifecycle', () => {
  it('one explicit pasted-invite join prepares missing local keys and retains the admission code for the destination', async () => {
    m.generate.mockImplementation(async () => { m.identity = local; });
    const h = setup({ phase: 'uninitialized', identity: null, ready: false });
    await waitFor(() => expect(m.pending).toHaveBeenCalled());
    await act(async () => h.result.current.handleJoinInvite(delivery));
    expect(m.generate).toHaveBeenCalledWith(1);
    expect(m.redeem).toHaveBeenCalledWith(delivery.server, delivery.code);
    expect(m.remote).toHaveBeenCalledWith(delivery.server);
    expect(h.result.current.code).toBe(delivery.code);
    expect(h.result.current.slug).toBe('destination');
    expect(h.result.current.serverReady).toBe(true);
  });

  it('offers a cold invite immediately and creates local keys only on Join', async () => {
    const registration = deferred<() => void>();
    m.listen.mockReturnValue(registration.promise); m.pending.mockResolvedValue(delivery);
    const h = setup({ phase: 'uninitialized', identity: null, ready: false });
    expect(m.pending).not.toHaveBeenCalled(); expect(h.result.current.protocolInviteLoading).toBe(true);
    await act(async () => registration.resolve(vi.fn()));
    expect(h.result.current.pendingProtocolInviteConfirmation).toEqual(delivery);
    expect(m.ack).toHaveBeenCalledWith(delivery.deliveryId);
    expect(m.redeem).not.toHaveBeenCalled(); expect(m.remote).not.toHaveBeenCalled();
    expect(m.generate).not.toHaveBeenCalled();
    m.generate.mockImplementation(async () => { m.identity = local; });
    await act(async () => h.result.current.handleAcceptProtocolInvite());
    expect(m.generate).toHaveBeenCalledWith(1);
    expect(m.redeem).toHaveBeenCalledWith(delivery.server, delivery.code);
  });

  it('disposes a listener that resolves after cleanup without draining or acknowledging the inbox', async () => {
    const registration = deferred<() => void>(); const unlisten = vi.fn();
    m.listen.mockReturnValue(registration.promise);
    const h = setup(); h.unmount();
    await act(async () => registration.resolve(unlisten));
    expect(unlisten).toHaveBeenCalledOnce(); expect(m.pending).not.toHaveBeenCalled(); expect(m.ack).not.toHaveBeenCalled();
  });

  it('does not acknowledge a read that resolves after unmount, so a new mount can recover it', async () => {
    const read = deferred<NativeInviteDelivery | null>(); m.pending.mockReturnValueOnce(read.promise);
    const h = setup(); await waitFor(() => expect(m.pending).toHaveBeenCalledOnce()); h.unmount();
    await act(async () => read.resolve(delivery)); expect(m.ack).not.toHaveBeenCalled();
    m.pending.mockResolvedValue(delivery); const next = setup();
    await waitFor(() => expect(next.result.current.pendingProtocolInviteConfirmation).toEqual(delivery));
    expect(m.ack).toHaveBeenCalledOnce();
  });

  it('a runtime notification during a cold read supersedes the old snapshot and duplicate notifications do not prompt again', async () => {
    const read = deferred<NativeInviteDelivery | null>(); m.pending.mockReturnValueOnce(read.promise);
    const h = setup(); await waitFor(() => expect(m.pending).toHaveBeenCalledOnce());
    const newer = { ...delivery, deliveryId: 'delivery-b', server: 'https://new.example' };
    m.pending.mockResolvedValue(newer);
    await act(async () => { notify(newer); read.resolve(delivery); });
    expect(h.result.current.pendingProtocolInviteConfirmation).toEqual(newer);
    expect(m.ack).toHaveBeenCalledWith(newer.deliveryId); expect(m.ack).not.toHaveBeenCalledWith(delivery.deliveryId);
    act(() => h.result.current.handleIgnoreProtocolInvite());
    await deliver(newer);
    expect(h.result.current.pendingProtocolInviteConfirmation).toBeNull(); expect(m.redeem).not.toHaveBeenCalled();
  });

  it('warm acceptance pauses registration until the exact invite context exists and prevents duplicate attempts', async () => {
    const redeem = deferred<{ valid: boolean; serverSlug: string }>(); m.redeem.mockReturnValue(redeem.promise);
    const h = setup({ phase: 'ready', identity: { ...local, pseudonymId: 'existing' }, ready: true });
    await waitFor(() => expect(m.pending).toHaveBeenCalled()); await deliver();
    let accepting!: Promise<void>;
    act(() => { accepting = h.result.current.handleAcceptProtocolInvite(); void h.result.current.handleAcceptProtocolInvite(); });
    expect(h.result.current.serverReady).toBe(false); expect(m.redeem).toHaveBeenCalledOnce(); expect(m.remote).not.toHaveBeenCalled();
    act(() => h.result.current.handleIgnoreProtocolInvite());
    expect(h.result.current.pendingProtocolInviteConfirmation).toEqual(delivery);
    await act(async () => { redeem.resolve({ valid: true, serverSlug: 'destination' }); await accepting; });
    expect(h.result.current.code).toBe(delivery.code); expect(h.result.current.slug).toBe('destination');
    expect(h.result.current.serverReady).toBe(true); expect(m.remote).toHaveBeenCalledWith(delivery.server);
    expect(m.saveMode).not.toHaveBeenCalled();
    h.rerender({ identity: m.identity, phase: 'ready' });
    await waitFor(() => expect(h.result.current.pendingProtocolInviteConfirmation).toBeNull());
    expect(m.saveMode).toHaveBeenCalledWith({ startup_mode: { mode: 'client', server_url: delivery.server } });
  });

  it('a validation outage keeps the previous session and the invitation for an explicit retry', async () => {
    m.redeem.mockRejectedValueOnce(new TypeError('Failed to fetch'));
    const h = setup({ phase: 'ready', identity: local, ready: true });
    await waitFor(() => expect(m.pending).toHaveBeenCalled()); await deliver();
    await act(async () => h.result.current.handleAcceptProtocolInvite());
    expect(h.result.current.pendingProtocolInviteConfirmation).toEqual(delivery);
    expect(h.result.current.protocolInviteError).toContain('Could not reach server'); expect(h.result.current.protocolInviteBusy).toBe(false);
    expect(h.result.current.serverReady).toBe(true); expect(m.setIdentity).not.toHaveBeenCalled(); expect(m.cleanup).not.toHaveBeenCalled();
    await act(async () => h.result.current.handleAcceptProtocolInvite());
    expect(m.redeem).toHaveBeenCalledTimes(2); expect(m.remote).toHaveBeenCalledOnce();
  });

  it('completes a saved-server switch to that server\'s different identity and accepts its optional root slash', async () => {
    const savedIdentity = { ...local, id: 'saved-target-identity', sk: '9876', pseudonymId: 'target-member' };
    const connection = deferred<{ id: string; identityId: string }>(); m.remote.mockReturnValue(connection.promise);
    const h = setup({ phase: 'ready', identity: local, ready: true });
    await waitFor(() => expect(m.pending).toHaveBeenCalled());
    const rootSlashInvite = { ...delivery, server: `${delivery.server}/` };
    await deliver(rootSlashInvite);
    let accepting!: Promise<void>;
    await act(async () => { accepting = h.result.current.handleAcceptProtocolInvite(); });
    expect(m.remote).toHaveBeenCalledWith(delivery.server);
    m.base = delivery.server; m.identity = savedIdentity;
    h.rerender({ phase: 'ready', identity: savedIdentity });
    expect(h.result.current.serverReady).toBe(false); expect(m.saveMode).not.toHaveBeenCalled();
    await act(async () => { connection.resolve({ id: 'saved-target', identityId: savedIdentity.id }); await accepting; });
    await waitFor(() => expect(h.result.current.pendingProtocolInviteConfirmation).toBeNull());
    expect(h.result.current.serverReady).toBe(true); expect(h.result.current.protocolInviteBusy).toBe(false);
    expect(h.result.current.code).toBeNull(); expect(h.result.current.slug).toBeNull();
    expect(m.saveMode).toHaveBeenCalledWith({ startup_mode: { mode: 'client', server_url: rootSlashInvite.server } });
    expect(m.register).not.toHaveBeenCalled(); expect(m.setIdentity).not.toHaveBeenCalled();
  });

  it.each(['different identity', 'different server', 'newer startup'] as const)(
    'a saved-server completion cannot enable registration after an unrelated %s selection', async change => {
      const connection = deferred<{ id: string; identityId: string }>(); m.remote.mockReturnValue(connection.promise);
      const h = setup(); await waitFor(() => expect(m.pending).toHaveBeenCalled()); await deliver();
      let accepting!: Promise<void>;
      await act(async () => { accepting = h.result.current.handleAcceptProtocolInvite(); });
      m.identity = { ...local, id: 'saved-target-identity', sk: '9876' }; m.base = delivery.server;
      if (change === 'different identity') m.identity = { ...m.identity, id: 'unrelated-identity' };
      if (change === 'different server') m.base = 'https://unrelated.example';
      if (change === 'newer startup') flow.current += 1;
      h.rerender({ phase: 'keys_ready', identity: m.identity });
      await act(async () => { connection.resolve({ id: 'saved-target', identityId: 'saved-target-identity' }); await accepting; });
      expect(h.result.current.serverReady).toBe(false); expect(h.result.current.protocolInviteBusy).toBe(false);
      expect(m.saveMode).not.toHaveBeenCalled(); expect(m.register).not.toHaveBeenCalled();
      expect(m.setIdentity).not.toHaveBeenCalled(); expect(m.cleanup).not.toHaveBeenCalled();
    },
  );

  it('the real registration hook cannot run in the middle of a warm remote-identity switch without its invite', async () => {
    const connection = deferred<{ id: string }>(); m.remote.mockReturnValue(connection.promise);
    const h = setup({ phase: 'ready', identity: local, ready: true }, true);
    await waitFor(() => expect(m.pending).toHaveBeenCalled()); await deliver();
    let accepting!: Promise<void>;
    await act(async () => { accepting = h.result.current.handleAcceptProtocolInvite(); });
    await waitFor(() => expect(m.remote).toHaveBeenCalledOnce());
    m.base = delivery.server; m.identity = { ...local, id: 'remote-identity' };
    h.rerender({ phase: 'keys_ready', identity: m.identity });
    expect(h.result.current.serverReady).toBe(false);
    expect(m.register).not.toHaveBeenCalled(); expect(m.summary).not.toHaveBeenCalled();
    await act(async () => { connection.resolve({ id: 'remote' }); await accepting; });
    await waitFor(() => expect(m.register).toHaveBeenCalledWith('destination', delivery.code, undefined));
    expect(m.register).toHaveBeenCalledOnce();
  });

  it('registration refusal retains the code and retry reuses the selected target identity instead of cloning again', async () => {
    const h = setup(); await waitFor(() => expect(m.pending).toHaveBeenCalled()); await deliver();
    await act(async () => h.result.current.handleAcceptProtocolInvite());
    m.error = 'Invite expired before registration'; h.rerender({ phase: 'error', identity: m.identity });
    await waitFor(() => expect(h.result.current.protocolInviteError).toBe(m.error));
    expect(h.result.current.pendingProtocolInviteConfirmation).toEqual(delivery); expect(h.result.current.code).toBe(delivery.code);
    await act(async () => h.result.current.handleAcceptProtocolInvite());
    expect(m.remote).toHaveBeenCalledOnce(); expect(m.setIdentity).toHaveBeenCalledWith({ phase: 'keys_ready', error: null, errorDetails: null });
  });

  it('refuses an unconfirmed successful HTTP response before changing server context', async () => {
    m.redeem.mockResolvedValue({ valid: false, serverSlug: 'destination' });
    const h = setup(); await waitFor(() => expect(m.pending).toHaveBeenCalled()); await deliver();
    await act(async () => h.result.current.handleAcceptProtocolInvite());
    expect(m.remote).not.toHaveBeenCalled(); expect(h.result.current.protocolInviteError).toContain('did not confirm');
    expect(h.result.current.code).toBeNull();
  });

  it('a connection refusal cannot leak the accepted invite code into the next manually chosen server', async () => {
    m.remote.mockResolvedValue(null); m.registrationError = 'Connection refused';
    const h = setup(); await waitFor(() => expect(m.pending).toHaveBeenCalled()); await deliver();
    await act(async () => h.result.current.handleAcceptProtocolInvite());
    expect(h.result.current.protocolInviteError).toBe('Connection refused');
    act(() => h.result.current.handleIgnoreProtocolInvite());
    expect(h.result.current.code).toBeNull(); expect(h.result.current.slug).toBeNull();
    expect(m.cleanup).not.toHaveBeenCalled();
  });

  it.each(['reject', 'resolve'] as const)('a stale validation %s cannot change a newer startup choice or clean its server', async outcome => {
    const redeem = deferred<{ valid: boolean; serverSlug: string }>(); m.redeem.mockReturnValue(redeem.promise);
    const h = setup(); await waitFor(() => expect(m.pending).toHaveBeenCalled()); await deliver();
    let accepting!: Promise<void>; act(() => { accepting = h.result.current.handleAcceptProtocolInvite(); });
    flow.current += 1;
    await act(async () => { if (outcome === 'reject') redeem.reject(new Error('old failure')); else redeem.resolve({ valid: true, serverSlug: 'old' }); await accepting; });
    expect(m.remote).not.toHaveBeenCalled(); expect(m.cleanup).not.toHaveBeenCalled(); expect(m.setIdentity).not.toHaveBeenCalled();
    expect(h.result.current.protocolInviteError).toBeNull(); expect(h.result.current.code).toBeNull(); expect(m.saveMode).not.toHaveBeenCalled();
  });

  it('logout during validation never restores the old identity or registers it', async () => {
    const redeem = deferred<{ valid: boolean; serverSlug: string }>(); m.redeem.mockReturnValue(redeem.promise);
    const h = setup(); await waitFor(() => expect(m.pending).toHaveBeenCalled()); await deliver();
    let accepting!: Promise<void>; act(() => { accepting = h.result.current.handleAcceptProtocolInvite(); });
    m.identity = null;
    await act(async () => { redeem.resolve({ valid: true, serverSlug: 'old' }); await accepting; });
    expect(m.remote).not.toHaveBeenCalled(); expect(m.setIdentity).not.toHaveBeenCalled(); expect(h.result.current.protocolInviteBusy).toBe(false);
  });
});
