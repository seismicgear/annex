/**
 * Wire a WebRTC session to incoming signaling frames on the WebSocket.
 *
 * The session itself is stored in a ref so the subscription always reads
 * the most recent session, even on the first render after a channel/ws
 * change (where state-based wiring would briefly point at the now-disconnected
 * previous session).
 */

import { useEffect, type MutableRefObject } from 'react';
import type { WebRtcSession } from '@/lib/webrtc';
import type { AnnexWebSocket } from '@/lib/ws';
import type { WsReceiveFrame } from '@/types';

export function useWebRtcSignals(
  ws: AnnexWebSocket | null,
  sessionRef: MutableRefObject<WebRtcSession | null>,
  channelId: string,
): void {
  useEffect(() => {
    if (!ws) return;
    return ws.onMessage((frame: WsReceiveFrame) => {
      const session = sessionRef.current;
      if (!session) return;
      if (frame.channelId !== channelId) return;

      if (frame.type === 'webrtc_answer' && frame.sdp) {
        session.handleAnswer(frame.sdp).catch((err) =>
          console.error('[webrtc] failed to handle answer:', err),
        );
      } else if (frame.type === 'webrtc_offer' && frame.sdp) {
        // A server-initiated offer: somebody joined or left the call and this
        // peer's track set changed. Answering is what makes the new tracks
        // live; ignoring it leaves the call frozen at the participants it
        // started with.
        session.handleRemoteOffer(frame.sdp).catch((err) =>
          console.error('[webrtc] failed to handle server offer:', err),
        );
      } else if (frame.type === 'webrtc_ice_candidate' && frame.candidate) {
        session.handleIceCandidate({
          candidate: frame.candidate,
          sdpMid: frame.sdpMid ?? undefined,
          sdpMLineIndex: frame.sdpMLineIndex ?? undefined,
        }).catch((err) =>
          console.error('[webrtc] failed to add ICE candidate:', err),
        );
      }
    });
  }, [ws, sessionRef, channelId]);
}
