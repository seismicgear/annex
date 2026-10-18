/**
 * Lifecycle hook for the WebRTC voice room.
 *
 * Creates a `WebRtcSession`, wires its connection-state and track-change
 * callbacks into React state, binds the WebSocket signaling subscription
 * (so server answers and ICE candidates reach the session), and tears
 * everything down on unmount or when channelId/ws changes.
 *
 * `iceServers` and `identity` are deliberately omitted from the create
 * effect's deps: the session is created once per (channelId, ws) pair and
 * not re-created if those props happen to change between renders.
 */

import { useEffect, useRef, useState } from 'react';
import { ConnectionState as RoomConnectionState, WebRtcSession } from '@/lib/webrtc';
import type { NativeConnectionState } from '@/lib/webrtc';
import type { AnnexWebSocket } from '@/lib/ws';
import { useVoiceStore } from '@/stores/voice';
import { useWebRtcSignals } from './useWebRtcSignals';

interface UseVoiceRoomArgs {
  channelId: string;
  iceServers: RTCIceServer[];
  identity: string;
  ws: AnnexWebSocket | null;
}

interface UseVoiceRoomResult {
  session: WebRtcSession | null;
  connectionState: NativeConnectionState;
  /** Incremented on every local/remote track change to trigger re-renders. */
  trackVersion: number;
}

export function useVoiceRoom({
  channelId,
  iceServers,
  identity,
  ws,
}: UseVoiceRoomArgs): UseVoiceRoomResult {
  const [session, setSession] = useState<WebRtcSession | null>(null);
  const [connectionState, setConnState] = useState<NativeConnectionState>(RoomConnectionState.Disconnected);
  const [trackVersion, setTrackVersion] = useState(0);
  const sessionRef = useRef<WebRtcSession | null>(null);

  useEffect(() => {
    if (!ws) return;

    const sess = new WebRtcSession(iceServers, channelId, identity, {
      // Read at send time, not capture time: the grant is stored by
      // `joinCall` and the session is constructed by an effect that can run
      // either side of it. Reading through `getState()` means the offer
      // carries whatever grant is current when it actually leaves, rather
      // than whatever was in the store when this closure was built.
      sendOffer: (ch, sdp) =>
        ws.sendWebRtcOffer(ch, sdp, useVoiceStore.getState().voiceToken),
      ...(useVoiceStore.getState().serverOffers ? {
        connect: (ch: string) => ws.sendWebRtcConnect(ch, useVoiceStore.getState().voiceToken),
      } : {}),
      sendAnswer: (ch, sdp) => ws.sendWebRtcAnswer(ch, sdp),
      sendMediaState: (ch, generation, state) => ws.sendWebRtcMediaState(ch, generation, state),
      sendIceCandidate: (ch, candidate, sdpMid, sdpMLineIndex) =>
        ws.sendIceCandidate(ch, candidate, sdpMid, sdpMLineIndex),
    });

    sess.onConnectionStateChange = (state) => setConnState(state);
    sess.onRemoteTracksChanged = () => setTrackVersion((v) => v + 1);
    sess.onLocalTrackChanged = () => setTrackVersion((v) => v + 1);

    sessionRef.current = sess;
    const unsubscribeStatus = ws.onStatus((_connected, state) => {
      if (state !== 'taken_over' && state !== 'revoked') return;
      // Stop tracks immediately, including capture requests still awaiting a
      // permission prompt. Do not wait for React's provider unmount to run.
      sessionRef.current = null;
      sess.disconnect();
      setSession(null);
    });
    // Do not mount media reconciliation while connect() is acquiring its
    // initial microphone: that starts a second capture and can unmute a call
    // the user joined muted. Keep the saved preference for the first capture.
    const initialMedia = useVoiceStore.getState();
    const unsubscribeInitialMute = useVoiceStore.subscribe((state, previous) => {
      if (state.micMuted && !previous.micMuted && sessionRef.current === sess) {
        void sess.setMicrophoneEnabled(false).catch(() => {});
      }
    });

    // Initiate the connection.
    //
    // A rejection has to drive the state machine, not just reach the console.
    // `connect()` sets `Connecting` as its first statement and only attaches
    // `pc.onconnectionstatechange` once the RTCPeerConnection exists, so a
    // failure before that — getUserMedia refused, createOffer throwing, the
    // signalling send failing — leaves the session pinned at `Connecting`
    // with no further state change ever arriving. The panel renders that as
    // "Joining…", so the user waits on a call that will never connect, with
    // no error and no way out but reloading.
    //
    // Setting `Disconnected` hands it to the mapping in VoiceRoomProvider,
    // which turns a disconnect during 'connecting' into
    // `handleUnexpectedDisconnect` — the path that already exists for a call
    // that drops, and the one a failed join should have been taking.
    sess.connect({ microphoneEnabled: !initialMedia.micMuted, inputDeviceId: initialMedia.inputDeviceId }).then(() => {
      unsubscribeInitialMute();
      if (sessionRef.current !== sess) return;
      useVoiceStore.getState().setMicMuted(!sess.isMicrophoneEnabled);
      setSession(sess);
    }).catch((err) => {
      unsubscribeInitialMute();
      if (sessionRef.current !== sess) return;
      console.error('[webrtc] connection failed:', err);
      sess.disconnect();
      setConnState(RoomConnectionState.Disconnected);
      useVoiceStore.getState().handleUnexpectedDisconnect('Voice connection failed. Try joining again.');
    });

    return () => {
      unsubscribeStatus();
      sessionRef.current = null;
      unsubscribeInitialMute();
      setSession(null);
      sess.disconnect();
    };
    // Deliberately omit iceServers/identity — session is created once per mount
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [channelId, ws]);

  useWebRtcSignals(ws, sessionRef, channelId);

  return { session, connectionState, trackVersion };
}
