/**
 * Root application component.
 *
 * Orchestrates the startup flow:
 *   Screen 1  – Identity creation (offline, zero network requests, no server)
 *   Screen 2  – Server / startup-mode selection (server starts here, not before)
 *
 * Identity creation is purely local — keys are generated and stored on the
 * device. No server is started, contacted, or registered with during
 * identity creation. The server only enters the picture when the user
 * explicitly chooses a server mode on Screen 2.
 *
 * Cross-cutting orchestration lives in the `useApp*` hooks under this
 * folder; rendering is delegated to StartupGate and MainLayout.
 */

import { useEffect, useRef, useState } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import { useServersStore } from '@/stores/servers';
import type { DegradedStartupInfo } from '@/components/StartupModeSelector';
import { clearWebStartupMode } from '@/lib/startup-prefs';
import { clearStartupMode as clearTauriStartupMode, isTauri } from '@/lib/tauri';
import { cancelMembershipProofGeneration, isProofGenerationInFlight } from '@/lib/zk';

import { MainLayout, type AppView } from './MainLayout';
import { StartupGate } from './StartupGate';
import { useAppBootstrap } from './useAppBootstrap';
import { useInviteFlow } from './useInviteFlow';
import { useNotificationPermission } from './useNotificationPermission';
import { useServerSelection } from './useServerSelection';
import { useSessionConnection } from './useSessionConnection';

/** Reconnection banner — shown when the WebSocket disconnects.
 * Uses Zustand subscription to track connection state transitions
 * without violating React strict-mode lint rules.
 */
export function ReconnectionBanner() {
  const wsConnected = useChannelsStore((s) => s.wsConnected);
  // Initialise from the current ws state at mount so a session that boots
  // already-disconnected shows the banner without a setState-in-effect.
  const [banner, setBanner] = useState<'hidden' | 'disconnected' | 'reconnected'>(
    () => (useChannelsStore.getState().wsConnected ? 'hidden' : 'disconnected'),
  );

  // Subscribe to wsConnected changes at the store level to detect transitions
  useEffect(() => {
    let wasConnected = useChannelsStore.getState().wsConnected;
    const unsub = useChannelsStore.subscribe((state) => {
      const nowConnected = state.wsConnected;
      if (!nowConnected && wasConnected) {
        setBanner('disconnected');
      } else if (nowConnected && !wasConnected) {
        setBanner('reconnected');
      }
      wasConnected = nowConnected;
    });
    return unsub;
  }, []);

  // Auto-hide the "Reconnected" banner after 2 seconds
  useEffect(() => {
    if (banner === 'reconnected') {
      const timer = setTimeout(() => setBanner('hidden'), 2000);
      return () => clearTimeout(timer);
    }
  }, [banner]);

  if (banner === 'hidden') return null;

  return (
    <div className={`reconnection-banner ${banner}`} role="alert">
      {wsConnected
        ? 'Reconnected'
        : 'Connection lost — reconnecting...'}
    </div>
  );
}

export default function App() {
  const {
    phase,
    identity,
    error,
    errorDetails,
    loadIdentities,
    loadPermissions,
    permissions,
    permissionsStatus,
    proofInFlight,
    provingStatus,
    registerWithServer,
  } = useIdentityStore();
  const { connectWs, disconnectWs, selectChannel, joinChannel, loadChannels } = useChannelsStore();
  const wsConnected = useChannelsStore((s) => s.wsConnected);
  const activeChannelId = useChannelsStore((s) => s.activeChannelId);
  const loadedChannels = useChannelsStore((s) => s.channels);
  const { servers, loadServers, saveCurrentServer, fetchServerImage } = useServersStore();
  const activeServer = useServersStore((s) => s.getActiveServer());
  const serverImageUrl = useServersStore((s) => s.serverImageUrl);
  const inTauri = isTauri();

  // Cross-cutting state owned by AppShell so multiple hooks/views can read it.
  const [serverReady, setServerReady] = useState(false);
  const [degradedStartup, setDegradedStartup] = useState<DegradedStartupInfo | null>(null);
  const [activeView, setActiveView] = useState<AppView>('chat');
  const [passwordRequired, setPasswordRequired] = useState(false);
  const [serverPassword, setServerPassword] = useState('');
  const [pendingInviteCode, setPendingInviteCode] = useState<string | null>(null);
  const [pendingServerSlug, setPendingServerSlug] = useState<string | null>(null);
  const startupFlowId = useRef(0);
  const canModerate = permissions?.capabilities.can_moderate === true;

  const beginStartupFlow = () => {
    startupFlowId.current += 1;
    return startupFlowId.current;
  };

  // Bootstrap (loadIdentities + first-run cleanup), startup-error mirror,
  // and proof-in-flight sync poll.
  const {
    identityChecked,
    startupInitError,
    startupErrorDetails,
    setStartupErrorDetails,
    provingFailures,
    setProvingFailures,
    retryBootstrap,
  } = useAppBootstrap({
    phase,
    error,
    errorDetails,
    proofInFlight,
    loadIdentities,
    loadServers,
    inTauri,
  });

  const resetToServerSelection = async () => {
    beginStartupFlow();
    if (isProofGenerationInFlight()) {
      await cancelMembershipProofGeneration('Proof generation cancelled before retry.');
    }

    // Clear persisted startup preferences so the user lands on the chooser
    // instead of auto-resuming the mode that just failed.
    if (isTauri()) {
      clearTauriStartupMode().catch(() => {});
    }
    clearWebStartupMode();

    useIdentityStore.setState({ phase: 'keys_ready', proofInFlight: false, provingStatus: 'idle', error: null, errorDetails: null });
    setStartupErrorDetails(null);
    setProvingFailures(0);
    setPasswordRequired(false);
    setServerPassword('');
    setServerReady(false);
  };

  // Invite flow: legacy URL invites and Tauri deep-link protocol invites.
  const {
    pendingInvite,
    pendingProtocolInvite,
    pendingProtocolInviteConfirmation,
    handleAcceptProtocolInvite,
    handleIgnoreProtocolInvite,
  } = useInviteFlow({
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
  });

  // Server selection and registration orchestration.
  useServerSelection({
    phase,
    identity,
    serverReady,
    inTauri,
    registerWithServer,
    saveCurrentServer,
    pendingInviteCode,
    pendingServerSlug,
    serverPassword,
    startupFlowId,
    setServerReady,
    setPasswordRequired,
    setServerPassword,
    setPendingInviteCode,
    setPendingServerSlug,
    setStartupErrorDetails,
    setDegradedStartup,
  });

  // WebSocket connect + token refresh while ready.
  useSessionConnection({
    phase,
    pseudonymId: identity?.pseudonymId,
    connectWs,
    disconnectWs,
    loadPermissions,
    fetchServerImage,
  });

  // Notification permission prompt once chat is fully active.
  useNotificationPermission({
    phase,
    pseudonymId: identity?.pseudonymId,
    wsConnected,
    loadedChannelsLength: loadedChannels.length,
    activeChannelId,
  });

  // Apply persona isolation — dynamic CSS custom properties per server context
  useEffect(() => {
    const raw = activeServer?.accentColor ?? '#e63946';
    const accentColor = /^#[0-9a-fA-F]{6}$/.test(raw) ? raw : '#e63946';
    document.documentElement.style.setProperty('--persona-accent', accentColor);

    const r = parseInt(accentColor.slice(1, 3), 16);
    const g = parseInt(accentColor.slice(3, 5), 16);
    const b = parseInt(accentColor.slice(5, 7), 16);
    document.documentElement.style.setProperty(
      '--persona-bg-tint',
      `rgba(${r}, ${g}, ${b}, 0.06)`,
    );
    document.documentElement.style.setProperty(
      '--persona-border-tint',
      `rgba(${r}, ${g}, ${b}, 0.3)`,
    );
  }, [activeServer?.accentColor]);

  // ────────────────────────────────────────────────────────────────────
  // RENDER GATES — evaluated top-to-bottom, first match wins.
  // The StartupGate component itself decides which sub-gate to render.
  // ────────────────────────────────────────────────────────────────────

  const showStartupGate =
    !identityChecked
    || (!!startupInitError && phase === 'error' && !identity?.sk)
    || !identity?.sk
    || !serverReady
    || phase !== 'ready'
    || !identity?.pseudonymId;

  if (showStartupGate) {
    return (
      <StartupGate
        identityChecked={identityChecked}
        startupInitError={startupInitError}
        startupErrorDetails={startupErrorDetails}
        errorDetails={errorDetails}
        phase={phase}
        error={error}
        identity={identity}
        pendingInvite={pendingInvite}
        pendingProtocolInvite={pendingProtocolInvite}
        pendingProtocolInviteConfirmation={pendingProtocolInviteConfirmation}
        handleAcceptProtocolInvite={handleAcceptProtocolInvite}
        handleIgnoreProtocolInvite={handleIgnoreProtocolInvite}
        serverReady={serverReady}
        passwordRequired={passwordRequired}
        serverPassword={serverPassword}
        setServerPassword={setServerPassword}
        proofInFlight={proofInFlight}
        provingStatus={provingStatus}
        provingFailures={provingFailures}
        beginStartupFlow={beginStartupFlow}
        setServerReady={setServerReady}
        setDegradedStartup={setDegradedStartup}
        setPasswordRequired={setPasswordRequired}
        resetToServerSelection={resetToServerSelection}
        retryBootstrap={retryBootstrap}
      />
    );
  }

  return (
    <MainLayout
      activeView={activeView}
      setActiveView={setActiveView}
      activeServer={activeServer}
      servers={servers}
      serverImageUrl={serverImageUrl}
      canModerate={canModerate}
      permissionsStatus={permissionsStatus}
      loadPermissions={loadPermissions}
      pendingProtocolInviteConfirmation={pendingProtocolInviteConfirmation}
      handleAcceptProtocolInvite={handleAcceptProtocolInvite}
      handleIgnoreProtocolInvite={handleIgnoreProtocolInvite}
      degradedStartup={degradedStartup}
      setDegradedStartup={setDegradedStartup}
      reconnectionBanner={<ReconnectionBanner />}
    />
  );
}
