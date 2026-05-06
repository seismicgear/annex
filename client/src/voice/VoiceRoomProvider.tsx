/**
 * WebRTC voice room provider and the in-room composition.
 *
 * VoiceRoomProvider owns the lifecycle (via useVoiceRoom), exposes the
 * session/connection-state through React context, and renders RoomContent
 * inside the `<div data-testid="webrtc-room">` element that the rest of
 * the panel UI nests under.
 *
 * RoomContent is the in-room composite: it pumps WebRTC connection
 * transitions back into the voice store, holds the keepalive + Tauri
 * media-restore lifecycle, manages the Layer-3 "Resume Sharing" recovery
 * banner, and renders the participant grid, controls, status pills, and
 * remote audio sinks.
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { ConnectionState as RoomConnectionState, type NativeConnectionState, type WebRtcSession } from '@/lib/webrtc';
import { isTauri, setMediaKeepalive, type PlatformMediaStatus } from '@/lib/tauri';
import { useChannelsStore } from '@/stores/channels';
import { useVoiceStore } from '@/stores/voice';
import { MediaControls } from './MediaControls';
import { LocalSelfView, ParticipantGrid, ScreenShareView } from './RemoteParticipants';
import { LocalMediaStatus } from './VoiceDiagnostics';
import { mediaErrorMessage, useTauriMediaRestore } from './useLocalMedia';
import { useRemoteAudio } from './useRemoteAudio';
import { useVoiceRoom } from './useVoiceRoom';

// ── WebRTC React Context ──

interface WebRtcContextValue {
  session: WebRtcSession;
  connectionState: NativeConnectionState;
  /** Incremented on every local/remote track change to trigger re-renders. */
  trackVersion: number;
}

const WebRtcContext = createContext<WebRtcContextValue | null>(null);

function useWebRtcContext(): WebRtcContextValue {
  const ctx = useContext(WebRtcContext);
  if (!ctx) throw new Error('useWebRtcContext must be used inside VoiceRoomProvider');
  return ctx;
}

// ── Custom hooks (matching the component API of the previous transport layer) ──

function useLocalParticipant() {
  const { session, trackVersion } = useWebRtcContext();
  void trackVersion; // subscribe to track changes
  return {
    localParticipant: session,
    isMicrophoneEnabled: session.isMicrophoneEnabled,
    isCameraEnabled: session.isCameraEnabled,
    isScreenShareEnabled: session.isScreenShareEnabled,
  };
}

function useConnectionState(): NativeConnectionState {
  const { connectionState } = useWebRtcContext();
  return connectionState;
}

// ── Remote audio renderer (replaces RoomAudioRenderer) ──

function RemoteAudioRenderer() {
  const { session, trackVersion } = useWebRtcContext();
  void trackVersion;
  const tracks = session.remoteAudioTracks;

  return (
    <>
      {tracks.map((rt) => (
        <RemoteAudioElement key={rt.id} stream={rt.stream} />
      ))}
    </>
  );
}

function RemoteAudioElement({ stream }: { stream: MediaStream }) {
  const ref = useRef<HTMLAudioElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.srcObject = stream;
    return () => { el.srcObject = null; };
  }, [stream]);

  return <audio ref={ref} autoPlay data-webrtc-remote="" />;
}

// ── In-room composite ──

interface RoomContentProps {
  onLeave: () => void;
  mediaStatus: PlatformMediaStatus | null;
  platformWarnings: string[];
}

function RoomContent({ onLeave, mediaStatus, platformWarnings }: RoomContentProps) {
  const { localParticipant, isMicrophoneEnabled, isCameraEnabled, isScreenShareEnabled } = useLocalParticipant();
  const nativeConnectionState = useConnectionState();
  const { setConnectionState, handleUnexpectedDisconnect, connectionState: storeConnectionState } = useVoiceStore();
  const [screenShareInterrupted, setScreenShareInterrupted] = useState(false);
  // Track whether the user intentionally left (via the leave button).
  const intentionalLeaveRef = useRef(false);

  // Sync WebRTC connection state to the voice store
  useEffect(() => {
    switch (nativeConnectionState) {
      case RoomConnectionState.Connected:
        setConnectionState('connected');
        break;
      case RoomConnectionState.Connecting:
      case RoomConnectionState.Reconnecting:
        setConnectionState('connecting');
        break;
      case RoomConnectionState.Disconnected:
        // Distinguish intentional leave from unexpected disconnect.
        // If the user clicked "Leave", the store is already cleared by leaveCall().
        // Otherwise, this is an unexpected disconnect — clean up stale session state.
        if (intentionalLeaveRef.current) {
          setConnectionState('idle');
        } else if (storeConnectionState === 'connected' || storeConnectionState === 'connecting') {
          handleUnexpectedDisconnect('Voice disconnected — the connection was lost.');
        }
        break;
    }
  }, [nativeConnectionState, setConnectionState, handleUnexpectedDisconnect, storeConnectionState]);

  // Wrap onLeave so the disconnect handler knows this was intentional.
  const handleLeaveInternal = useCallback(() => {
    intentionalLeaveRef.current = true;
    onLeave();
  }, [onLeave]);

  // Layer 1: Tell the Rust backend to keep the webview alive during the call.
  // This prevents WebView2 from setting IsVisible=false on minimize, which
  // would kill MediaStreamTracks.
  useEffect(() => {
    if (!isTauri()) return;
    setMediaKeepalive(true).catch(() => {});
    return () => { setMediaKeepalive(false).catch(() => {}); };
  }, []);

  // Layer 2: Safety-net hook that restores any tracks that still died.
  // If screen share can't auto-restart, it fires the callback for Layer 3.
  const handleScreenShareInterrupted = useCallback(() => {
    setScreenShareInterrupted(true);
  }, []);
  useTauriMediaRestore(localParticipant, handleScreenShareInterrupted);

  // Apply voice store output prefs (device, volume, deafen) to remote audio.
  useRemoteAudio();

  // The interrupted banner auto-hides when screen share re-enables (see
  // showScreenShareInterrupted below), so no effect/ref clearing is needed.
  const showScreenShareInterrupted = screenShareInterrupted && !isScreenShareEnabled;

  // Error state for screen share resume failures
  const [resumeError, setResumeError] = useState<string | null>(null);

  // Layer 3: Resume banner — the button click provides the user gesture
  // needed for getDisplayMedia() in browsers that require it.
  const resumeScreenShare = useCallback(async () => {
    setResumeError(null);
    try {
      await localParticipant.setScreenShareEnabled(true);
      // Only clear the interrupted banner after success
      setScreenShareInterrupted(false);
    } catch (err) {
      // Distinguish user-cancel from actual runtime failure
      if (err instanceof DOMException && err.name === 'AbortError') {
        const isLikelyUserCancel = !err.message || /user/i.test(err.message) || err.message === 'AbortError';
        if (isLikelyUserCancel) {
          // User cancelled the picker — keep the recovery affordance visible
          return;
        }
      }
      // Real failure — surface error but keep the banner visible for retry
      setResumeError(mediaErrorMessage(err, 'Resume screen share'));
    }
  }, [localParticipant]);

  return (
    <>
      <RemoteAudioRenderer />
      {showScreenShareInterrupted && (
        <div className="screen-share-interrupted" role="alert">
          <span>{resumeError ?? 'Screen share was interrupted'}</span>
          <button onClick={resumeScreenShare} className="screen-share-resume-btn">
            Resume Sharing
          </button>
          <button
            onClick={() => { setScreenShareInterrupted(false); setResumeError(null); }}
            className="screen-share-dismiss-btn"
            aria-label="Dismiss"
          >
            &times;
          </button>
        </div>
      )}
      <LocalMediaStatus
        isMicrophoneEnabled={isMicrophoneEnabled}
        isCameraEnabled={isCameraEnabled}
        isScreenShareEnabled={isScreenShareEnabled}
      />
      <LocalSelfView session={localParticipant} />
      <ScreenShareView />
      <ParticipantGrid session={localParticipant} />
      <MediaControls
        localParticipant={localParticipant}
        isMicrophoneEnabled={isMicrophoneEnabled}
        isCameraEnabled={isCameraEnabled}
        isScreenShareEnabled={isScreenShareEnabled}
        onLeave={handleLeaveInternal}
        mediaStatus={mediaStatus}
        platformWarnings={platformWarnings}
      />
    </>
  );
}

// ── Provider ──

interface VoiceRoomProviderProps {
  channelId: string;
  iceServers: RTCIceServer[];
  identity: string;
  onLeave: () => void;
  mediaStatus: PlatformMediaStatus | null;
  platformWarnings: string[];
  /**
   * Optional override for when callers want to render their own room UI
   * inside the provider. By default RoomContent is rendered.
   */
  children?: ReactNode;
}

/**
 * WebRTC voice room provider. Creates a WebRtcSession, wires signaling
 * via the existing AnnexWebSocket, and exposes the session through React
 * context. By default it renders the in-room composite (RoomContent);
 * pass `children` to override.
 */
export function VoiceRoomProvider({
  channelId,
  iceServers,
  identity,
  onLeave,
  mediaStatus,
  platformWarnings,
  children,
}: VoiceRoomProviderProps) {
  const ws = useChannelsStore((s) => s.ws);
  const { session, connectionState, trackVersion } = useVoiceRoom({
    channelId,
    iceServers,
    identity,
    ws,
  });

  if (!session) return null;

  return (
    <WebRtcContext.Provider value={{ session, connectionState, trackVersion }}>
      <div data-testid="webrtc-room">
        {children ?? (
          <RoomContent
            onLeave={onLeave}
            mediaStatus={mediaStatus}
            platformWarnings={platformWarnings}
          />
        )}
      </div>
    </WebRtcContext.Provider>
  );
}
