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
      sendOffer: (ch, sdp) => ws.sendWebRtcOffer(ch, sdp),
      sendIceCandidate: (ch, candidate, sdpMid, sdpMLineIndex) =>
        ws.sendIceCandidate(ch, candidate, sdpMid, sdpMLineIndex),
    });

    sess.onConnectionStateChange = (state) => setConnState(state);
    sess.onRemoteTracksChanged = () => setTrackVersion((v) => v + 1);
    sess.onLocalTrackChanged = () => setTrackVersion((v) => v + 1);

    sessionRef.current = sess;
    setSession(sess);

    // Initiate the connection
    sess.connect().catch((err) =>
      console.error('[webrtc] connection failed:', err),
    );

    return () => {
      sessionRef.current = null;
      setSession(null);
      sess.disconnect();
    };
    // Deliberately omit iceServers/identity — session is created once per mount
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [channelId, ws]);

  useWebRtcSignals(ws, sessionRef, channelId);

  return { session, connectionState, trackVersion };
}
