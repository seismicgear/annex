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
import { redeemInvite } from '@/lib/api';
import { clearInviteFromUrl, parseLegacyInviteFromUrl } from '@/lib/invite';
import {
  getPendingInvite,
  listenForInvite,
  saveStartupMode,
} from '@/lib/tauri';
import type { IdentityPhase } from '@/stores/identity';
import type { InvitePayload, LegacyInvitePayload, StoredIdentity } from '@/types';

interface UseInviteFlowArgs {
  phase: IdentityPhase;
  identity: StoredIdentity | null;
  inTauri: boolean;
  joinChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  loadChannels: (pseudonymId: string) => Promise<void>;
  selectChannel: (pseudonymId: string, channelId: string) => Promise<void>;
  startupFlowId: MutableRefObject<number>;
  beginStartupFlow: () => number;
  setServerReady: Dispatch<SetStateAction<boolean>>;
  setPendingInviteCode: Dispatch<SetStateAction<string | null>>;
  setPendingServerSlug: Dispatch<SetStateAction<string | null>>;
  setActiveView: Dispatch<SetStateAction<'chat' | 'federation' | 'events' | 'admin-policy' | 'admin-channels' | 'admin-members' | 'admin-server'>>;
}

interface UseInviteFlowResult {
  pendingInvite: LegacyInvitePayload | null;
  pendingProtocolInvite: InvitePayload | null;
  pendingProtocolInviteConfirmation: InvitePayload | null;
  handleAcceptProtocolInvite: () => Promise<void>;
  handleIgnoreProtocolInvite: () => void;
}

export function useInviteFlow({
  phase,
  identity,
  inTauri,
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
  const [pendingProtocolInvite, setPendingProtocolInvite] = useState<InvitePayload | null>(null);
  const [pendingProtocolInviteConfirmation, setPendingProtocolInviteConfirmation] = useState<InvitePayload | null>(null);
  const inviteProcessed = useRef(false);

  // ── Listen for annex:// deep-link invite events (Tauri only) ──
  // Also fetch any buffered cold-start invite that arrived before this
  // listener mounted. The Rust backend buffers it in managed state so
  // it's not lost if the React tree hasn't rendered yet.
  useEffect(() => {
    if (!inTauri) return;
    let unlisten: (() => void) | null = null;

    // Fetch buffered cold-start invite (arrives before listener mounts)
    getPendingInvite()
      .then((invite) => {
        if (invite) setPendingProtocolInvite(invite);
      })
      .catch(() => {}); // Non-fatal — runtime listener covers future invites

    // Runtime listener for subsequent deep-link events
    listenForInvite((invite) => {
      setPendingProtocolInvite(invite);
    }).then((fn) => { unlisten = fn; })
      .catch(() => {}); // Non-fatal — deep-link listener unavailable
    return () => { unlisten?.(); };
  }, [inTauri]);

  // ── Process protocol invite: validate, switch server, trigger registration ──
  useEffect(() => {
    if (!pendingProtocolInvite || !identity?.sk) return;
    if (phase !== 'keys_ready' && phase !== 'ready') return;
    if (phase === 'ready') {
      setPendingProtocolInviteConfirmation(pendingProtocolInvite);
      setPendingProtocolInvite(null);
      return;
    }
    let cancelled = false;
    const flowId = beginStartupFlow();
    const isCurrentFlow = () => startupFlowId.current === flowId;

    (async () => {
      try {
        // 1. Validate invite code with target server
        let redeemResult;
        try {
          redeemResult = await redeemInvite(pendingProtocolInvite.server, pendingProtocolInvite.code);
        } catch (fetchErr) {
          if (cancelled || !isCurrentFlow()) return;
          // Distinguish network errors from server-side rejections
          const isNetworkError = fetchErr instanceof TypeError && /failed to fetch/i.test(fetchErr.message);
          const message = isNetworkError
            ? `Could not reach server at ${pendingProtocolInvite.server}. Check your connection and try again.`
            : (fetchErr instanceof Error ? fetchErr.message : 'Invite validation failed');
          useIdentityStore.setState({ phase: 'error', error: message });
          setPendingProtocolInvite(null);
          return;
        }
        if (cancelled || !isCurrentFlow()) return;

        // 2. Add remote server, clone identity, and switch API target
        const { beginRemoteRegistration } = useServersStore.getState();
        const server = await beginRemoteRegistration(pendingProtocolInvite.server);
        if (!server) {
          if (cancelled || !isCurrentFlow()) return;
          useIdentityStore.setState({
            phase: 'error',
            error: `Failed to connect to server at ${pendingProtocolInvite.server}.`,
          });
          useServersStore.getState().cleanupFailedRegistration();
          setPendingProtocolInvite(null);
          return;
        }

        // 3. Persist startup preference so returning users auto-connect
        if (inTauri) {
          saveStartupMode({
            startup_mode: { mode: 'client', server_url: pendingProtocolInvite.server },
          }).catch(() => {});
        }
        if (!isCurrentFlow()) return;

        // 4. Store invite code + slug for registration, mark server ready
        // (beginRemoteRegistration already reset phase to keys_ready)
        setPendingInviteCode(pendingProtocolInvite.code);
        setPendingServerSlug(redeemResult.serverSlug);
        setServerReady(true);
        setPendingProtocolInvite(null);
      } catch (err) {
        if (cancelled || !isCurrentFlow()) return;
        useIdentityStore.setState({
          phase: 'error',
          error: err instanceof Error ? err.message : 'Invite validation failed',
        });
        useServersStore.getState().cleanupFailedRegistration();
        setPendingProtocolInvite(null);
      }
    })();

    return () => { cancelled = true; };
  }, [
    pendingProtocolInvite,
    identity?.sk,
    phase,
    inTauri,
    beginStartupFlow,
    startupFlowId,
    setServerReady,
    setPendingInviteCode,
    setPendingServerSlug,
  ]);

  const handleAcceptProtocolInvite = async () => {
    if (!pendingProtocolInviteConfirmation) return;
    const flowId = beginStartupFlow();
    const isCurrentFlow = () => startupFlowId.current === flowId;
    const invite = pendingProtocolInviteConfirmation;
    setPendingProtocolInviteConfirmation(null);
    try {
      const redeemResult = await redeemInvite(invite.server, invite.code);
      if (!isCurrentFlow()) return;
      const { beginRemoteRegistration } = useServersStore.getState();
      const server = await beginRemoteRegistration(invite.server);
      if (!isCurrentFlow()) return;
      if (!server) {
        useIdentityStore.setState({
          phase: 'error',
          error: `Failed to connect to server at ${invite.server}.`,
        });
        useServersStore.getState().cleanupFailedRegistration();
        return;
      }
      if (inTauri) {
        saveStartupMode({
          startup_mode: { mode: 'client', server_url: invite.server },
        }).catch(() => {});
      }
      if (!isCurrentFlow()) return;
      setPendingInviteCode(invite.code);
      setPendingServerSlug(redeemResult.serverSlug);
      setServerReady(true);
    } catch (err) {
      const isNetworkError = err instanceof TypeError && /failed to fetch/i.test(err.message);
      const message = isNetworkError
        ? `Could not reach server at ${invite.server}. Check your connection and try again.`
        : (err instanceof Error ? err.message : 'Invite validation failed');
      useIdentityStore.setState({ phase: 'error', error: message });
      useServersStore.getState().cleanupFailedRegistration();
    }
  };

  const handleIgnoreProtocolInvite = () => {
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
    handleAcceptProtocolInvite,
    handleIgnoreProtocolInvite,
  };
}
