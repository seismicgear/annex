/**
 * Top-level voice panel.
 *
 * Drives the disconnected → connected state machine, surfaces voice
 * permission gating, polls the call-active status to decide between
 * "Create Call" and "Join Call", and mounts the WebRTC room provider
 * when there is an active voice session.
 *
 * The actual in-room UI (controls, participants, status pills, audio
 * sinks) is rendered by VoiceRoomProvider.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import * as api from '@/lib/api';
import { getPlatformMediaStatus, isTauri, type PlatformMediaStatus } from '@/lib/tauri';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import { useServersStore } from '@/stores/servers';
import { useVoiceStore } from '@/stores/voice';
import { VoiceCaptions } from './VoiceCaptions';
import { VoiceRoomProvider } from './VoiceRoomProvider';
import { PlatformMediaWarning } from './VoiceDiagnostics';

export function VoicePanel() {
  const identity = useIdentityStore((s) => s.identity);
  const permissions = useIdentityStore((s) => s.permissions);
  const activeChannelId = useChannelsStore((s) => s.activeChannelId);
  const channels = useChannelsStore((s) => s.channels);
  const activeServerId = useServersStore((s) => s.activeServerId);

  const {
    voiceToken,
    webrtcUrl,
    iceServers,
    connectedChannelId,
    joinCall,
    leaveCall,
    checkCallActive,
    isCallActive,
    isJoining,
    getJoinError,
    clearChannelCallState,
    joiningAnyCall,
    lastFailedChannelId,
    dismissConnectionError,
  } = useVoiceStore();
  const permissionsStatus = useIdentityStore((s) => s.permissionsStatus);
  const loadPermissions = useIdentityStore((s) => s.loadPermissions);
  const voiceSessionDisabled = useVoiceStore((s) => s.voiceSessionDisabled);
  const voiceSessionDisabledReason = useVoiceStore((s) => s.voiceSessionDisabledReason);
  const [retryingPermissions, setRetryingPermissions] = useState(false);

  // Track the server ID that was active when the call was joined.
  // Capture only on session establishment (transition from no token to active token),
  // not on every activeServerId update, to prevent a later server switch from
  // overwriting the stored value.
  const callServerIdRef = useRef<string | null>(null);
  const prevTokenRef = useRef<string | null>(null);
  useEffect(() => {
    const hadToken = !!prevTokenRef.current;
    const hasToken = !!voiceToken;
    prevTokenRef.current = voiceToken;
    // Only set the ref when a new session starts (no token → token)
    if (!hadToken && hasToken && connectedChannelId) {
      callServerIdRef.current = activeServerId;
    }
    // Clear when the session ends
    if (!hasToken) {
      callServerIdRef.current = null;
    }
  }, [voiceToken, connectedChannelId, activeServerId]);

  // Guard: if the active server changed since we joined, the token is stale.
  const isTokenStale = !!(
    voiceToken &&
    connectedChannelId &&
    callServerIdRef.current !== null &&
    callServerIdRef.current !== activeServerId
  );

  // Query platform media capabilities, refreshable on window focus / panel open.
  const [mediaStatus, setMediaStatus] = useState<PlatformMediaStatus | null>(null);
  const refreshMediaStatus = useCallback(() => {
    if (!isTauri()) return;
    getPlatformMediaStatus()
      .then((status) => setMediaStatus(status))
      .catch(() => { /* non-fatal: desktop-only command */ });
  }, []);

  // Initial load + refresh on window focus / app resume
  useEffect(() => {
    refreshMediaStatus();
    const onFocus = () => refreshMediaStatus();
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [refreshMediaStatus]);

  const platformWarnings = mediaStatus?.warnings ?? [];

  // Retry permissions on window focus when in error state
  useEffect(() => {
    if (permissionsStatus !== 'error') return;
    const onFocus = () => { void loadPermissions(); };
    window.addEventListener('focus', onFocus);
    return () => window.removeEventListener('focus', onFocus);
  }, [permissionsStatus, loadPermissions]);

  const handleRetryPermissions = useCallback(async () => {
    setRetryingPermissions(true);
    try {
      await loadPermissions();
    } finally {
      setRetryingPermissions(false);
    }
  }, [loadPermissions]);

  const activeChannel = channels.find((c) => c.channel_id === activeChannelId);
  const isVoiceCapable =
    activeChannel?.channel_type === 'Voice' || activeChannel?.channel_type === 'Hybrid';
  // Only allow voice when permissions have been positively resolved as true.
  // Do NOT default to true when permissions are absent — require explicit confirmation.
  const isVoiceAllowed = permissions?.capabilities.can_voice === true;
  const permissionsLoading = permissionsStatus === 'loading';
  const permissionsError = permissionsStatus === 'error';
  const permissionsDenied = permissionsStatus === 'denied';
  const permissionsReady = permissionsStatus === 'ready';
  // Ask the server whether voice can work here BEFORE offering the button.
  // The readiness check is server-global, so one request covers every voice
  // channel; `loadVoiceConfig` no-ops once resolved and is reset on server
  // switch by `forceReset`.
  const voiceConfig = useVoiceStore((s) => s.voiceConfig);
  const voiceConfigStatus = useVoiceStore((s) => s.voiceConfigStatus);
  const loadVoiceConfig = useVoiceStore((s) => s.loadVoiceConfig);
  useEffect(() => {
    if (!isVoiceCapable) return;
    void loadVoiceConfig();
  }, [isVoiceCapable, loadVoiceConfig]);

  // Only a POSITIVE answer that the server is not ready blocks the button. A
  // failed or pending check says nothing, and must not turn into a spurious
  // "voice is unavailable" on a server where it works.
  const serverVoiceUnready =
    voiceConfigStatus === 'ready' &&
    voiceConfig !== null &&
    !(voiceConfig.policy_enabled && voiceConfig.infrastructure_ready);

  // Voice join requires: permissions loaded successfully AND can_voice === true
  // AND voice was not disabled at startup AND the server can actually serve it.
  const canJoinVoice =
    isVoiceAllowed && permissionsReady && !voiceSessionDisabled && !serverVoiceUnready;

  // Clear stale call state when switching away from a channel so the next
  // channel does not inherit the previous channel's status or errors.
  const prevChannelRef = useRef<string | null>(null);
  useEffect(() => {
    const prev = prevChannelRef.current;
    prevChannelRef.current = activeChannelId;
    if (prev && prev !== activeChannelId) {
      clearChannelCallState(prev);
    }
  }, [activeChannelId, clearChannelCallState]);

  // Derive per-channel join-in-progress, call-active status, and join error.
  const joining = activeChannelId ? isJoining(activeChannelId) : false;
  const callActive = activeChannelId ? isCallActive(activeChannelId) : false;
  const lastJoinErrorDetails = activeChannelId ? getJoinError(activeChannelId) : null;
  const lastJoinError = lastJoinErrorDetails?.display ?? null;

  // Poll voice status: before a call, to choose between "Create" and "Join";
  // during one, to keep the participant roster current.
  //
  // This used to stop the moment `voiceToken` was set — that is, the moment
  // you were actually in the call. That was fine when the response was only a
  // count used to label a button, but the roster now names the tiles, so
  // freezing it at the pre-join poll meant the person who created a call held
  // an empty roster forever and nobody who joined afterwards ever appeared.
  // The poll continues while connected; it is one small request every ten
  // seconds against a route the client already calls.
  //
  // Polling is the right shape here rather than join/leave events, because a
  // participant can also vanish without an event the client sees — a dropped
  // connection, a reaped peer — and a poll converges on the truth either way.
  //
  // Two channels are polled, not one. The channel being *looked at* decides
  // whether the button reads "Create Call" or "Join Call". The channel whose
  // call you are *in* keeps the roster that names the tiles, and those are
  // only the same channel until you click another one to read something
  // while staying in the call — at which point the previous version stopped
  // polling the call entirely and the roster froze.
  const pollChannels = useMemo(() => {
    const ids: string[] = [];
    if (isVoiceCapable && activeChannelId) ids.push(activeChannelId);
    if (connectedChannelId && connectedChannelId !== activeChannelId) {
      ids.push(connectedChannelId);
    }
    return ids;
  }, [isVoiceCapable, activeChannelId, connectedChannelId]);
  // Joined into a primitive so the effect below re-runs when the SET changes
  // rather than on every render — an array literal is a new reference each
  // time and would restart the interval continuously.
  const pollKey = pollChannels.join(',');

  useEffect(() => {
    const pseudonymId = identity?.pseudonymId;
    if (!pseudonymId || !pollKey) return;
    const ids = pollKey.split(',');

    const poll = () => ids.forEach((id) => checkCallActive(pseudonymId, id));
    poll();
    const interval = setInterval(poll, 10_000);
    return () => clearInterval(interval);
  }, [pollKey, identity?.pseudonymId, checkCallActive]);

  const pseudonymId = identity?.pseudonymId ?? null;

  const handleJoin = useCallback(async () => {
    if (!pseudonymId || !activeChannelId || !canJoinVoice) return;
    await joinCall(pseudonymId, activeChannelId);
  }, [pseudonymId, activeChannelId, canJoinVoice, joinCall]);

  const handleLeave = useCallback(async () => {
    if (!pseudonymId) return;
    await leaveCall(pseudonymId);
  }, [pseudonymId, leaveCall]);

  // Use setup hint from structured error, or fetch from server as fallback.
  const [setupHint, setSetupHint] = useState<string | null>(null);
  useEffect(() => {
    // Prefer the structured setup_hint from the join response
    if (lastJoinErrorDetails?.setupHint) {
      setSetupHint(lastJoinErrorDetails.setupHint);
      return;
    }

    // Readiness is known up front now, so its hint can be shown without a
    // failed join first.
    if (serverVoiceUnready && voiceConfig?.setup_hint) {
      setSetupHint(voiceConfig.setup_hint);
      return;
    }

    const isVoiceNotConfigured =
      lastJoinErrorDetails?.code === 'voice_not_configured' ||
      lastJoinError?.includes('not configured');

    if (!isVoiceNotConfigured) {
      setSetupHint(null);
      return;
    }

    let cancelled = false;

    api.getVoiceConfigStatus()
      .then((status) => {
        if (cancelled || (status.policy_enabled && status.infrastructure_ready)) return;
        setSetupHint(status.setup_hint);
      })
      .catch(() => {
        // Best-effort: if the status endpoint fails, just show the raw error
      });

    return () => {
      cancelled = true;
    };
  }, [lastJoinError, lastJoinErrorDetails, serverVoiceUnready, voiceConfig]);

  // Build RTCIceServer array from the server-provided config.
  const rtcIceServers = useMemo(() => {
    return (iceServers ?? []).map((s) => ({
      urls: s.urls,
      username: s.username || undefined,
      credential: s.credential || undefined,
    }));
  }, [iceServers]);

  const connectionState = useVoiceStore((s) => s.connectionState);
  const connectionError = useVoiceStore((s) => s.connectionError);

  // If connected to a call, always show the WebRTC room (even on non-voice channels).
  // But NOT if the token is stale from a server switch, and NOT if the
  // connection has failed (session state was already cleared by the store).
  if (voiceToken && webrtcUrl && connectedChannelId && !isTokenStale) {
    // Find the channel name for the connected call
    const connectedChannel = channels.find((c) => c.channel_id === connectedChannelId);
    const channelLabel = connectedChannel?.name ?? connectedChannelId.slice(0, 12);

    const headerText = connectionState === 'connecting'
      ? 'Joining...'
      : connectionState === 'failed'
        ? 'Voice Disconnected'
        : 'Voice Connected';

    // A <section> with a name, not a bare <div>: the call is a distinct region
    // of the page, and a screen-reader user navigating by landmark otherwise
    // has no way to jump to it — or to tell where it ends and the message
    // history begins. axe reports the bare div as `region`.
    return (
      <section className="voice-panel connected" aria-label="Voice call">
        <div className="voice-connected-header">
          {headerText} — <strong>{channelLabel}</strong>
        </div>
        {connectionError && (
          <div className="voice-error" role="alert">
            <p>{connectionError}</p>
          </div>
        )}
        <VoiceCaptions />
        <VoiceRoomProvider
          channelId={connectedChannelId}
          iceServers={rtcIceServers}
          identity={pseudonymId ?? 'unknown'}
          onLeave={handleLeave}
          mediaStatus={mediaStatus}
          platformWarnings={platformWarnings}
        />
      </section>
    );
  }

  // Show a persistent disconnect banner even on non-voice channels so the
  // user can see that a previous voice call was unexpectedly disconnected.
  const failedChannel = lastFailedChannelId
    ? channels.find((c) => c.channel_id === lastFailedChannelId)
    : null;
  if (!isVoiceCapable || !activeChannelId) {
    // Even on text channels, show a disconnect recovery banner if applicable
    if (connectionError && lastFailedChannelId) {
      return (
        <div className="voice-panel disconnected voice-disconnect-banner">
          <div className="voice-error" role="alert">
            <p>
              Voice disconnected{failedChannel ? ` from ${failedChannel.name}` : ''}: {connectionError}
            </p>
            <button onClick={dismissConnectionError} className="media-error-dismiss" aria-label="Dismiss">&times;</button>
          </div>
        </div>
      );
    }
    return null;
  }

  const buttonText = joining
    ? 'Joining...'
    : callActive
      ? 'Join Call'
      : 'Create Call';

  // Most specific first: a server that cannot do voice at all is more useful
  // to say than anything about this identity's permissions on it.
  const unavailableReason = serverVoiceUnready
    ? voiceConfig?.policy_enabled === false
      ? 'Voice is turned off for this server.'
      : 'Voice is not set up on this server yet.'
    : voiceSessionDisabled
      ? (voiceSessionDisabledReason ?? 'Voice is not available for this session.')
      : permissionsDenied
        ? 'Voice is not allowed for your identity on this server.'
        : permissionsError
          ? null // Handled by the inline retry UI below
          : !permissionsReady
            ? 'Checking voice permissions…'
            : !isVoiceAllowed
              ? 'Voice is disabled by server policy for your identity.'
              : null;

  // Show connectionError when the active channel matches the last failed channel
  const showConnectionError = !!(connectionError && lastFailedChannelId && lastFailedChannelId === activeChannelId);
  // Distinguish join-time vs dropped-call errors for the user
  const connectionErrorLabel = connectionState === 'failed' && showConnectionError
    ? connectionError
    : null;

  return (
    <div className="voice-panel disconnected">
      <PlatformMediaWarning mediaStatus={mediaStatus} />
      {permissionsLoading && (
        <div className="voice-permissions-notice" role="status">Checking voice permissions…</div>
      )}
      {permissionsError && (
        <div className="voice-permissions-notice voice-error" role="status">
          Could not verify voice permissions right now.
          <button
            type="button"
            className="inline-action-btn"
            onClick={handleRetryPermissions}
            disabled={retryingPermissions}
          >
            {retryingPermissions ? 'Retrying...' : 'Retry'}
          </button>
        </div>
      )}
      {voiceSessionDisabled && (
        <div className="voice-permissions-notice voice-error" role="status">
          {voiceSessionDisabledReason ?? 'Voice is not available for this session.'}
        </div>
      )}
      <button
        onClick={handleJoin}
        disabled={joining || !canJoinVoice || joiningAnyCall || retryingPermissions}
        className="voice-join-btn"
        title={unavailableReason ?? (permissionsLoading ? 'Checking voice permissions…' : undefined)}
      >
        {buttonText}
      </button>
      {(lastJoinError || unavailableReason || connectionErrorLabel) && (
        <div className="voice-error" role="alert">
          {connectionErrorLabel && !lastJoinError && <p>{connectionErrorLabel}</p>}
          {lastJoinError && <p>{lastJoinError}</p>}
          {!lastJoinError && !connectionErrorLabel && unavailableReason && <p>{unavailableReason}</p>}
          {setupHint && <p className="voice-setup-hint">{setupHint}</p>}
          {connectionErrorLabel && (
            <button onClick={dismissConnectionError} className="media-error-dismiss" aria-label="Dismiss">&times;</button>
          )}
        </div>
      )}
    </div>
  );
}
