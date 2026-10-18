/**
 * Invite handling for both URL-based legacy invites and Tauri deep-link
 * protocol invites. Owns the pending-invite state machine and exposes
 * accept/ignore handlers for the confirmation banner.
 *
 * The legacy URL invite path joins a channel after the identity is ready;
 * the deep-link path validates with the target server and primes the
 * registration flow with the invite code and server slug.
 */

import { useEffect, useRef, useState, type Dispatch, type MutableRefObject, type SetStateAction } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useServersStore } from '@/stores/servers';
import { getApiBaseUrl, redeemInvite } from '@/lib/api';
import { clearInviteFromUrl, parseLegacyInviteFromUrl } from '@/lib/invite';
import {
  getPendingInvite,
  ackPendingInvite,
  listenForInvite,
  saveStartupMode,
} from '@/lib/tauri';
import type { NativeInviteDelivery } from '@/lib/tauri';
import type { IdentityPhase } from '@/stores/identity';
import type { AppView } from '@/app/MainLayout';
import type { InvitePayload, LegacyInvitePayload, StoredIdentity } from '@/types';

interface UseInviteFlowArgs {
  phase: IdentityPhase;
  identity: StoredIdentity | null;
  inTauri: boolean;
  serverReady: boolean;
  joinChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  loadChannels: (pseudonymId: string) => Promise<void>;
  selectChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  startupFlowId: MutableRefObject<number>;
  beginStartupFlow: () => number;
  setServerReady: Dispatch<SetStateAction<boolean>>;
  setPendingInviteCode: Dispatch<SetStateAction<string | null>>;
  setPendingServerSlug: Dispatch<SetStateAction<string | null>>;
  setActiveView: Dispatch<SetStateAction<AppView>>;
}

interface UseInviteFlowResult {
  pendingInvite: LegacyInvitePayload | null;
  pendingProtocolInvite: InvitePayload | null;
  pendingProtocolInviteConfirmation: InvitePayload | null;
  protocolInviteLoading: boolean;
  protocolInviteBusy: boolean;
  protocolInviteError: string | null;
  handleAcceptProtocolInvite: () => Promise<void>;
  handleJoinInvite: (invite: InvitePayload) => Promise<void>;
  handleIgnoreProtocolInvite: () => void;
}

export function useInviteFlow({
  phase,
  identity,
  inTauri,
  serverReady,
  joinChannel,
  loadChannels,
  selectChannel,
  startupFlowId,
  beginStartupFlow,
  setServerReady,
  setPendingInviteCode,
  setPendingServerSlug,
  setActiveView,
}: UseInviteFlowArgs): UseInviteFlowResult {
  const [pendingInvite, setPendingInvite] = useState<LegacyInvitePayload | null>(
    () => parseLegacyInviteFromUrl(),
  );
  const [pendingProtocolInvite, setPendingProtocolInvite] = useState<NativeInviteDelivery | null>(null);
  const [pendingProtocolInviteConfirmation, setPendingProtocolInviteConfirmation] = useState<InvitePayload | null>(null);
  const [protocolInviteLoading, setProtocolInviteLoading] = useState(inTauri);
  const [protocolInviteBusy, setProtocolInviteBusy] = useState(false);
  const [protocolInviteError, setProtocolInviteError] = useState<string | null>(null);
  const accepting = useRef(false);
  const mounted = useRef(false);
  const seenDelivery = useRef<string | null>(null);
  const prepared = useRef<{ invite: InvitePayload; identityId: string; serverId: string; flowId: number } | null>(null);
  const inviteProcessed = useRef(false);

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  // ── Listen for annex:// deep-link invite events (Tauri only) ──
  // Also fetch any buffered cold-start invite that arrived before this
  // listener mounted. The Rust backend buffers it in managed state so
  // it's not lost if the React tree hasn't rendered yet.
  useEffect(() => {
    if (!inTauri) return;
    let disposed = false;
    let unlisten: (() => void) | null = null;
    let draining = false;
    let requested = false;
    const drain = async () => {
      requested = true;
      if (draining || disposed) return;
      draining = true;
      try {
        while (requested && !disposed) {
          requested = false;
          const invite = await getPendingInvite();
          // A notification during the read means this snapshot may already
          // be stale. Read again before presenting or acknowledging it.
          if (!disposed && !requested && invite && seenDelivery.current !== invite.deliveryId) {
            seenDelivery.current = invite.deliveryId;
            setPendingProtocolInvite(invite);
          }
        }
      } catch {
        // The native inbox retains unacknowledged deliveries for a later read.
      } finally {
        draining = false;
        if (!disposed) setProtocolInviteLoading(false);
      }
    };
    void (async () => {
      try {
        const fn = await listenForInvite(() => { void drain(); });
        if (disposed) { fn(); return; }
        unlisten = fn;
      } catch {
        // Cold delivery is still recoverable if event registration fails.
      }
      if (!disposed) await drain();
    })();
    return () => { disposed = true; unlisten?.(); };
  }, [inTauri]);

  useEffect(() => {
    if (pendingProtocolInvite) void ackPendingInvite(pendingProtocolInvite.deliveryId).catch(() => {});
  }, [pendingProtocolInvite]);

  // ── Protocol invite: always ask before joining a server ──
  //
  // An `annex://` link names the server it will register you with. Accepting
  // one silently is a consent decision made on the user's behalf, using an
  // identifier an attacker chose.
  //
  // Both phases now route to the same confirmation banner. Previously only
  // `ready` (already registered on some server) did; a cold start at
  // `keys_ready` — keys generated, not yet registered — ran the whole flow
  // straight through: redeem the code, register the identity with that
  // server, persist it as the startup mode. The identical link therefore
  // asked for confirmation on a warm app and asked for nothing on a fresh
  // install, which is the wrong way round: a fresh install is exactly when
  // the user has no relationship with any server to judge the invite
  // against.
  //
  // `handleAcceptProtocolInvite` performs the same steps the auto-path did,
  // so accepting the banner is behaviour-identical — the only difference is
  // that a person decided.
  useEffect(() => {
    if (!pendingProtocolInvite || !identity?.sk || protocolInviteBusy) return;
    if (phase !== 'keys_ready' && phase !== 'ready' && phase !== 'error') return;
    setPendingProtocolInviteConfirmation(pendingProtocolInvite);
    setProtocolInviteError(null);
    prepared.current = null;
    setPendingProtocolInvite(null);
  }, [pendingProtocolInvite, identity?.sk, phase, protocolInviteBusy]);

  // Keep the accepted destination available through proof/registration errors.
  // Only a ready identity in that exact server context completes the invite.
  useEffect(() => {
    const attempt = prepared.current;
    if (!serverReady || !attempt || startupFlowId.current !== attempt.flowId || identity?.id !== attempt.identityId) return;
    if (phase === 'error') {
      accepting.current = false;
      setProtocolInviteBusy(false);
      setProtocolInviteError(useIdentityStore.getState().error || 'Joining this server failed. Try the invite again.');
    } else if (phase === 'ready' && getApiBaseUrl().replace(/\/+$/, '') === attempt.invite.server.replace(/\/+$/, '')) {
      accepting.current = false;
      prepared.current = null;
      setProtocolInviteBusy(false);
      setProtocolInviteError(null);
      setPendingProtocolInviteConfirmation(null);
      setPendingInviteCode(null);
      setPendingServerSlug(null);
      if (inTauri) void saveStartupMode({ startup_mode: { mode: 'client', server_url: attempt.invite.server } }).catch(() => {});
    }
  }, [phase, identity?.id, inTauri, startupFlowId, serverReady, setPendingInviteCode, setPendingServerSlug]);

  const acceptProtocolInvite = async (invite: InvitePayload, sourceIdentity: StoredIdentity) => {
    if (accepting.current || !sourceIdentity.sk) return;
    accepting.current = true;
    setPendingProtocolInviteConfirmation(invite);
    setProtocolInviteBusy(true);
    setProtocolInviteError(null);
    const flowId = beginStartupFlow();
    const sourceKey = sourceIdentity.sk;
    const sourceIdentityId = sourceIdentity.id;
    const sourceBase = getApiBaseUrl();
    let expectedIdentity = sourceIdentity;
    let expectedBase = sourceBase;
    const ownsAttempt = () => mounted.current && startupFlowId.current === flowId;
    const isCurrentFlow = () => ownsAttempt()
      && useIdentityStore.getState().identity?.id === expectedIdentity.id
      && useIdentityStore.getState().identity?.sk === expectedIdentity.sk
      && getApiBaseUrl().replace(/\/+$/, '') === expectedBase.replace(/\/+$/, '');
    const destination = invite.server.replace(/\/+$/, '');
    const wasReady = serverReady;
    // A warm app must not auto-register the cloned identity before its invite
    // code and destination slug have been installed.
    setServerReady(false);
    try {
      const redeemResult = await redeemInvite(invite.server, invite.code);
      if (!isCurrentFlow()) return;
      if (redeemResult.valid !== true || !redeemResult.serverSlug?.trim()) {
        throw new Error('The server did not confirm this invite. Try again or request a new link.');
      }
      setPendingInviteCode(invite.code);
      setPendingServerSlug(redeemResult.serverSlug);
      const prior = prepared.current;
      if (prior && prior.identityId === useIdentityStore.getState().identity?.id
        && prior.invite.server === invite.server && prior.invite.code === invite.code
        && getApiBaseUrl().replace(/\/+$/, '') === destination) {
        prior.flowId = flowId;
        useIdentityStore.setState({ phase: 'keys_ready', error: null, errorDetails: null });
        setServerReady(true);
        return;
      }
      const { beginRemoteRegistration } = useServersStore.getState();
      const server = await beginRemoteRegistration(destination);
      if (!ownsAttempt()) return;
      if (!server) {
        if (!isCurrentFlow()) return;
        throw new Error(useServersStore.getState().registrationError || `Failed to connect to server at ${invite.server}.`);
      }
      const currentIdentity = useIdentityStore.getState().identity;
      // A saved server may own completely different keys from the identity
      // that accepted the invite. Only accept that server's exact identity,
      // or the source-key clone selected for this new registration. An
      // unrelated selection while the switch was pending must stay untouched.
      if (!currentIdentity || getApiBaseUrl().replace(/\/+$/, '') !== destination) return;
      if (server.identityId ? currentIdentity.id !== server.identityId
        : currentIdentity.sk !== sourceKey
          || useServersStore.getState().pendingRegistrationServerId !== server.id) return;
      expectedIdentity = currentIdentity;
      expectedBase = destination;
      prepared.current = { invite, identityId: currentIdentity.id, serverId: server.id, flowId };
      setServerReady(true);
    } catch (err) {
      if (!isCurrentFlow()) return;
      const isNetworkError = err instanceof TypeError && /failed to fetch/i.test(err.message);
      const message = isNetworkError
        ? `Could not reach server at ${invite.server}. Check your connection and try again.`
        : (err instanceof Error && err.message ? err.message : 'Invite validation failed. Try again.');
      setProtocolInviteError(message);
      if (!prepared.current) {
        setPendingInviteCode(null);
        setPendingServerSlug(null);
        setServerReady(wasReady && getApiBaseUrl() === sourceBase
          && useIdentityStore.getState().identity?.id === sourceIdentityId);
      }
      accepting.current = false;
      setProtocolInviteBusy(false);
    } finally {
      if (mounted.current && !isCurrentFlow()) {
        accepting.current = false;
        setProtocolInviteBusy(false);
      }
    }
  };

  // Connect/Join is the user's choice. Create local keys when needed and run
  // the same destination-bound redemption and registration as a deep link.
  const handleJoinInvite = async (invite: InvitePayload) => {
    if (accepting.current) return;
    if (!useIdentityStore.getState().identity?.sk) {
      await useIdentityStore.getState().generateLocalKeys(1);
    }
    const source = useIdentityStore.getState().identity;
    if (!source?.sk) throw new Error(useIdentityStore.getState().error || 'Your identity could not be prepared. Try again.');
    await acceptProtocolInvite(invite, source);
  };

  const handleAcceptProtocolInvite = async () => {
    if (pendingProtocolInviteConfirmation) await handleJoinInvite(pendingProtocolInviteConfirmation);
  };

  const handleIgnoreProtocolInvite = () => {
    if (accepting.current) return;
    const attempt = prepared.current;
    if (attempt && startupFlowId.current === attempt.flowId) {
      beginStartupFlow();
      setServerReady(false);
      setPendingInviteCode(null);
      setPendingServerSlug(null);
      if (useServersStore.getState().pendingRegistrationServerId === attempt.serverId) {
        void useServersStore.getState().cleanupFailedRegistration(attempt.serverId);
      }
      useIdentityStore.setState({ phase: 'keys_ready', error: null, errorDetails: null });
    }
    prepared.current = null;
    setProtocolInviteError(null);
    setPendingProtocolInviteConfirmation(null);
  };

  // Process legacy URL invite after identity is ready
  useEffect(() => {
    if (
      phase === 'ready' &&
      identity?.pseudonymId &&
      pendingInvite &&
      !inviteProcessed.current
    ) {
      inviteProcessed.current = true;
      const processInvite = async () => {
        try {
          await joinChannel(identity.pseudonymId!, pendingInvite.channelId).catch(() => {
            // Expected: channel might already be joined
          });
          await loadChannels(identity.pseudonymId!);
          selectChannel(identity.pseudonymId!, pendingInvite.channelId);
        } finally {
          clearInviteFromUrl();
          setActiveView('chat');
          setPendingInvite(null);
        }
      };
      processInvite().catch(() => {
        // Non-fatal: invite processing failed, user lands on chat view
      });
    }
  }, [phase, identity?.pseudonymId, pendingInvite, joinChannel, loadChannels, selectChannel, setActiveView]);

  return {
    pendingInvite,
    pendingProtocolInvite,
    pendingProtocolInviteConfirmation,
    protocolInviteLoading,
    protocolInviteBusy,
    protocolInviteError,
    handleAcceptProtocolInvite,
    handleJoinInvite,
    handleIgnoreProtocolInvite,
  };
}
