/**
 * A failed WebRTC connect must not leave the user waiting forever.
 *
 * `WebRtcSession.connect()` sets `Connecting` as its very first statement and
 * only attaches `pc.onconnectionstatechange` once the RTCPeerConnection has
 * been constructed. So a rejection before that point — getUserMedia refused,
 * createOffer throwing, the signalling send failing — leaves the session
 * pinned at `Connecting`, because no further state change is ever emitted.
 *
 * The rejection was caught and logged to a console nobody has open. VoicePanel
 * renders `connecting` as "Joining…", so the user sat on a call that could
 * never connect, with no error and no exit but a reload.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, waitFor } from '@testing-library/react';

const connectMock = vi.fn();
let lastSession: { onConnectionStateChange?: (s: string) => void } | undefined;

vi.mock('@/lib/webrtc', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/webrtc')>();
  return {
    ...actual,
    // A class, not `vi.fn().mockImplementation(...)`: the hook calls
    // `new WebRtcSession(...)`, and an arrow function is not a constructor.
    WebRtcSession: class {
      constructor() {
        lastSession = this as unknown as { onConnectionStateChange?: (s: string) => void };
      }
      connect = connectMock;
      disconnect = vi.fn();
      trackPublications = new Map();
      remoteVideoTracks: unknown[] = [];
      remoteAudioTracks: unknown[] = [];
      onConnectionStateChange?: (s: string) => void;
      onRemoteTracksChanged?: () => void;
      onLocalTrackChanged?: () => void;
    },
  };
});

vi.mock('./useWebRtcSignals', () => ({ useWebRtcSignals: vi.fn() }));

describe('useVoiceRoom', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('a rejected connect reports disconnected instead of hanging on connecting', async () => {
    // Faithful to the real `connect()`, which sets Connecting as its FIRST
    // statement and only then does the work that can throw. Without this the
    // test is vacuous: the hook's initial state is already 'disconnected', so
    // asserting that after a rejection passes whether or not the fix exists.
    // Verified by reverting the fix and watching this go red.
    connectMock.mockImplementationOnce(function (this: { onConnectionStateChange?: (s: string) => void }) {
      lastSession?.onConnectionStateChange?.('connecting');
      return Promise.reject(new Error('getUserMedia denied'));
    });
    const { useVoiceRoom } = await import('./useVoiceRoom');

    const ws = { sendWebRtcOffer: vi.fn(), sendWebRtcAnswer: vi.fn(), sendIceCandidate: vi.fn() };
    const { result } = renderHook(() =>
      useVoiceRoom({ channelId: 'chan', iceServers: [], identity: 'me', ws: ws as never }),
    );

    await waitFor(() => {
      expect(result.current.connectionState).toBe('disconnected');
    });
  });

  it('a successful connect still reports the session\'s own state', async () => {
    // The failure path must not clobber the success path. Asserting on a
    // *pending* connect would prove nothing: the hook's initial state is
    // already 'disconnected', so a slow connect and a failed one are
    // indistinguishable by state alone. What matters is that a session which
    // goes on to report Connected is reflected, rather than being overwritten
    // by the catch.
    const { WebRtcSession } = await import('@/lib/webrtc');
    vi.mocked(WebRtcSession as never);
    connectMock.mockResolvedValueOnce(undefined);

    const { useVoiceRoom } = await import('./useVoiceRoom');
    const ws = { sendWebRtcOffer: vi.fn(), sendWebRtcAnswer: vi.fn(), sendIceCandidate: vi.fn() };
    const { result } = renderHook(() =>
      useVoiceRoom({ channelId: 'chan2', iceServers: [], identity: 'me', ws: ws as never }),
    );

    await waitFor(() => expect(result.current.session).toBeTruthy());
    const emit = (result.current.session as unknown as { onConnectionStateChange?: (s: string) => void })
      .onConnectionStateChange;
    expect(emit).toBeTypeOf('function');
    emit!('connected');

    await waitFor(() => {
      expect(result.current.connectionState).toBe('connected');
    });
  });
});
