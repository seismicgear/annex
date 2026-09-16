/**
 * The saved device has to be requested as an EXACT constraint.
 *
 * `deviceId: '<id>'` is an *ideal* constraint. The browser uses that device
 * when it is present and silently substitutes any other when it is not — so a
 * microphone or camera the user chose in Audio & Video Settings and later
 * unplugged was quietly replaced, with nothing said, on call sites whose own
 * comments claim they "respect the selected device".
 *
 * It also made an existing recovery path dead code: `useLocalMedia` catches
 * `NotFoundError`/`OverconstrainedError` to raise "Saved camera not found or
 * disconnected" with a "Use default camera" button, and a bare constraint
 * raises neither, whichever camera is missing.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { WebRtcSession, type SignalingCallbacks } from './webrtc';

const getUserMedia = vi.fn();
const original = globalThis.navigator?.mediaDevices;

function fakeTrack(kind: 'audio' | 'video') {
  return { kind, stop: vi.fn(), enabled: true } as unknown as MediaStreamTrack;
}

/**
 * A session built with the four arguments `WebRtcSession` actually takes.
 *
 * These call sites used to read `new WebRtcSession({ url, token } as never)` —
 * a LiveKit room-options bag left over from before the native SFU, cast to
 * `never`, which is what let three missing arguments go unnoticed. Nothing in
 * these tests reaches signalling: the mic and camera paths touch only `pc` and
 * `navigator.mediaDevices`.
 */
function newSession(): WebRtcSession {
  const signaling: SignalingCallbacks = {
    sendOffer: vi.fn(),
    sendAnswer: vi.fn(),
    sendIceCandidate: vi.fn(),
  };
  return new WebRtcSession([], 'chan-1', 'p1', signaling);
}

/** A peer connection stub with only what `setCameraEnabled` touches. */
function attachFakePc(session: WebRtcSession) {
  const pc = {
    addTrack: vi.fn(() => ({ replaceTrack: vi.fn() })),
    addTransceiver: vi.fn(),
    getSenders: vi.fn(() => []),
  };
  (session as unknown as { pc: unknown }).pc = pc;
  return pc;
}

beforeEach(() => {
  getUserMedia.mockReset();
  getUserMedia.mockImplementation(async (c: MediaStreamConstraints) => ({
    getAudioTracks: () => [fakeTrack('audio')],
    getVideoTracks: () => [fakeTrack('video')],
    _c: c,
  }));
  Object.defineProperty(globalThis.navigator, 'mediaDevices', {
    value: { getUserMedia },
    configurable: true,
  });
});
afterEach(() => {
  Object.defineProperty(globalThis.navigator, 'mediaDevices', {
    value: original,
    configurable: true,
  });
});

describe('saved device selection', () => {
  it('asks for the saved microphone exactly, so a missing one is an error not a substitution', async () => {
    const s = newSession();
    attachFakePc(s);
    await s.setMicrophoneEnabled(true, { deviceId: 'mic-7' });

    const audio = getUserMedia.mock.calls[0][0].audio as MediaTrackConstraints;
    expect(audio.deviceId).toEqual({ exact: 'mic-7' });
  });

  it('asks for the saved camera exactly, which is what makes the recovery prompt reachable', async () => {
    const s = newSession();
    attachFakePc(s);
    await s.setCameraEnabled(true, { deviceId: 'cam-3' });

    const video = getUserMedia.mock.calls[0][0].video as MediaTrackConstraints;
    expect(video.deviceId).toEqual({ exact: 'cam-3' });
  });

  it('sends no deviceId at all when nothing is saved', async () => {
    const s = newSession();
    attachFakePc(s);
    await s.setMicrophoneEnabled(true);

    const audio = getUserMedia.mock.calls[0][0].audio as MediaTrackConstraints;
    expect(audio.deviceId).toBeUndefined();
  });
});
