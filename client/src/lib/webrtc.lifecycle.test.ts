import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { TrackSource, WebRtcSession } from './webrtc';

function track(kind: 'audio' | 'video') {
  const value = {
    kind, enabled: true, readyState: 'live', onended: null as (() => void) | null,
    stop: vi.fn(() => { value.readyState = 'ended'; }),
  };
  return value;
}

function stream(mediaTrack: ReturnType<typeof track>): MediaStream {
  return {
    getTracks: () => [mediaTrack],
    getAudioTracks: () => mediaTrack.kind === 'audio' ? [mediaTrack] : [],
    getVideoTracks: () => mediaTrack.kind === 'video' ? [mediaTrack] : [],
  } as unknown as MediaStream;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((done) => { resolve = done; });
  return { promise, resolve };
}

const getUserMedia = vi.fn();
const getDisplayMedia = vi.fn();
const originalDevices = navigator.mediaDevices;

class PeerConnection {
  connectionState: RTCPeerConnectionState = 'new';
  onconnectionstatechange: (() => void) | null = null;
  ontrack: ((event: RTCTrackEvent) => void) | null = null;
  transceivers: { kind: string; direction?: RTCRtpTransceiverDirection; sender: { track: MediaStreamTrack | null; replaceTrack: ReturnType<typeof vi.fn> } }[] = [];
  addTransceiver = vi.fn((kind: string, init?: RTCRtpTransceiverInit) => {
    const sender = {
      track: null as MediaStreamTrack | null,
      replaceTrack: vi.fn(async (value: MediaStreamTrack | null) => { sender.track = value; }),
    };
    const transceiver = { kind, direction: init?.direction, sender };
    this.transceivers.push(transceiver);
    return transceiver;
  });
  addTrack = vi.fn();
  getTransceivers = vi.fn(() => this.transceivers);
  createOffer = vi.fn(async () => ({ type: 'offer', sdp: 'offer-sdp' }));
  createAnswer = vi.fn(async () => ({ type: 'answer', sdp: 'answer-sdp' }));
  setRemoteDescription = vi.fn<(description: RTCSessionDescriptionInit) => Promise<void>>().mockResolvedValue(undefined);
  addIceCandidate = vi.fn<(candidate: RTCIceCandidateInit) => Promise<void>>().mockResolvedValue(undefined);
  setLocalDescription = vi.fn(async () => {});
  close = vi.fn();
}

let pc: PeerConnection;
let session: WebRtcSession;
let sendOffer = vi.fn<(channelId: string, sdp: string) => void>();
let sendAnswer = vi.fn<(channelId: string, sdp: string) => void>();

beforeEach(() => {
  getUserMedia.mockReset();
  getDisplayMedia.mockReset();
  pc = new PeerConnection();
  vi.stubGlobal('RTCPeerConnection', class { constructor() { return pc; } });
  Object.defineProperty(navigator, 'mediaDevices', {
    configurable: true, value: { getUserMedia, getDisplayMedia },
  });
  sendOffer = vi.fn();
  sendAnswer = vi.fn();
  session = new WebRtcSession([], 'room', 'member', {
    sendOffer, sendAnswer, sendIceCandidate: vi.fn(),
  });
});

afterEach(() => {
  session.disconnect();
  vi.useRealTimers();
  vi.unstubAllGlobals();
  Object.defineProperty(navigator, 'mediaDevices', { configurable: true, value: originalDevices });
});

const sources = [
  { name: 'microphone', source: TrackSource.Microphone, index: 0, kind: 'audio' as const,
    capture: getUserMedia, set: (value: boolean) => session.setMicrophoneEnabled(value) },
  { name: 'camera', source: TrackSource.Camera, index: 1, kind: 'video' as const,
    capture: getUserMedia, set: (value: boolean) => session.setCameraEnabled(value) },
  { name: 'screen', source: TrackSource.ScreenShare, index: 2, kind: 'video' as const,
    capture: getDisplayMedia, set: (value: boolean) => session.setScreenShareEnabled(value) },
];

describe('WebRTC publication lifecycle', () => {
  it('waits for the server offer and answers without reversing negotiation roles', async () => {
    const connect = vi.fn();
    session = new WebRtcSession([], 'room', 'member', {
      sendOffer, connect, sendAnswer, sendIceCandidate: vi.fn(),
    });
    await session.connect({ microphoneEnabled: false });
    expect(connect).toHaveBeenCalledWith('room');
    expect(sendOffer).not.toHaveBeenCalled();
    expect(pc.createOffer).not.toHaveBeenCalled();
    expect(pc.transceivers).toHaveLength(0);
    const offered = ['audio', 'video', 'video'].map((kind, index) =>
      Object.assign(pc.addTransceiver(kind), { mid: String(index) }));
    await session.handleRemoteOffer('server-initial-offer');
    await session.handleRemoteOffer('server-added-participant');
    expect(offered.map(item => item.direction)).toEqual(['sendonly', 'sendonly', 'sendonly']);
    expect(sendAnswer).toHaveBeenCalledTimes(2);
    expect(pc.createOffer).not.toHaveBeenCalled();
  });
  it('retains capture through temporary transport loss and cancels the recovery deadline on reconnect', async () => {
    vi.useFakeTimers();
    await session.connect({ microphoneEnabled: false });
    const camera = track('video');
    getUserMedia.mockResolvedValueOnce(stream(camera));
    await session.setCameraEnabled(true);
    pc.connectionState = 'connected';
    pc.onconnectionstatechange?.();
    pc.connectionState = 'disconnected';
    pc.onconnectionstatechange?.();
    expect(session.connectionState).toBe('reconnecting');
    await vi.advanceTimersByTimeAsync(20_000);
    expect(camera.stop).not.toHaveBeenCalled();
    expect(pc.close).not.toHaveBeenCalled();
    pc.connectionState = 'connected';
    pc.onconnectionstatechange?.();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(session.connectionState).toBe('connected');
    expect(session.isCameraEnabled).toBe(true);
  });

  it('ends a recovery that cannot reconnect and ignores a departed connection callback', async () => {
    vi.useFakeTimers();
    await session.connect({ microphoneEnabled: false });
    pc.connectionState = 'disconnected';
    const oldChange = pc.onconnectionstatechange;
    oldChange?.();
    await vi.advanceTimersByTimeAsync(30_000);
    expect(session.connectionState).toBe('disconnected');
    session.disconnect();
    pc.connectionState = 'connected';
    oldChange?.();
    expect(session.connectionState).toBe('disconnected');
  });

  for (const kind of ['audio', 'video'] as const) {
    it(`keeps one ${kind} publication when rejoin repeats a receiver event`, async () => {
      await session.connect({ microphoneEnabled: false });
      const remote = { ...track(kind), id: `${kind}-peer` };
      const event = { track: remote, streams: [stream(remote)] } as unknown as RTCTrackEvent;
      pc.ontrack?.(event);
      pc.ontrack?.(event);
      const publications = kind === 'audio' ? session.remoteAudioTracks : session.remoteVideoTracks;
      expect(publications).toHaveLength(1);
      expect(publications[0].track).toBe(remote);
    });

    it(`retains replacement ${kind} when an old receiver with the same ID ends late`, async () => {
      await session.connect({ microphoneEnabled: false });
      const previous = { ...track(kind), id: `${kind}-peer` };
      const replacement = { ...track(kind), id: `${kind}-peer` };
      pc.ontrack?.({ track: previous, streams: [stream(previous)] } as unknown as RTCTrackEvent);
      const staleEnd = previous.onended;
      pc.ontrack?.({ track: replacement, streams: [stream(replacement)] } as unknown as RTCTrackEvent);
      staleEnd?.();
      const publications = kind === 'audio' ? session.remoteAudioTracks : session.remoteVideoTracks;
      expect(publications).toHaveLength(1);
      expect(publications[0].track).toBe(replacement);
    });
  }

  it('applies the initial answer before a follow-up offer and drains early ICE in wire order', async () => {
    await session.connect({ microphoneEnabled: false });
    const earlyCandidate = { candidate: 'early', sdpMid: '0' };
    await session.handleIceCandidate(earlyCandidate);
    expect(pc.addIceCandidate).not.toHaveBeenCalled();
    const answerGate = deferred<void>();
    pc.setRemoteDescription.mockReturnValueOnce(answerGate.promise);
    const initialAnswer = session.handleAnswer('initial-answer');
    const followup = session.handleRemoteOffer('follow-up-offer');
    const laterCandidate = session.handleIceCandidate({ candidate: 'later', sdpMid: '3' });
    await Promise.resolve();
    expect(pc.setRemoteDescription).toHaveBeenCalledExactlyOnceWith({ type: 'answer', sdp: 'initial-answer' });
    expect(pc.createAnswer).not.toHaveBeenCalled();
    answerGate.resolve(undefined);
    await Promise.all([initialAnswer, followup, laterCandidate]);
    expect(pc.setRemoteDescription.mock.calls).toEqual([
      [{ type: 'answer', sdp: 'initial-answer' }], [{ type: 'offer', sdp: 'follow-up-offer' }],
    ]);
    expect(pc.addIceCandidate.mock.calls).toEqual([[earlyCandidate], [{ candidate: 'later', sdpMid: '3' }]]);
    expect(sendAnswer).toHaveBeenCalledExactlyOnceWith('room', 'answer-sdp');
  });

  it('discards queued offers when the call ends during SDP processing', async () => {
    await session.connect({ microphoneEnabled: false });
    const answerGate = deferred<void>();
    pc.setRemoteDescription.mockReturnValueOnce(answerGate.promise);
    const initialAnswer = session.handleAnswer('initial-answer');
    const followup = session.handleRemoteOffer('follow-up-offer');
    await Promise.resolve();
    session.disconnect();
    answerGate.resolve(undefined);
    await Promise.all([initialAnswer, followup]);
    expect(pc.setRemoteDescription).toHaveBeenCalledTimes(1);
    expect(pc.createAnswer).not.toHaveBeenCalled();
    expect(sendAnswer).not.toHaveBeenCalled();
  });

  it('negotiates independent camera and screen slots before either is enabled', async () => {
    await session.connect({ microphoneEnabled: false });
    expect(getUserMedia).not.toHaveBeenCalled();
    expect(pc.transceivers.map((entry) => entry.kind)).toEqual(['audio', 'video', 'video']);
    // Remote publications must arrive on server-created receiving sections,
    // where the track IDs identify their actual sender, even for a late join.
    expect(pc.transceivers.map((entry) => entry.direction)).toEqual(['sendonly', 'sendonly', 'sendonly']);
    const camera = track('video');
    const screen = track('video');
    getUserMedia.mockResolvedValueOnce(stream(camera));
    getDisplayMedia.mockResolvedValueOnce(stream(screen));
    await session.setCameraEnabled(true);
    await session.setScreenShareEnabled(true);
    expect(pc.transceivers[1].sender.track).toBe(camera);
    expect(pc.transceivers[2].sender.track).toBe(screen);
    expect(pc.addTrack).not.toHaveBeenCalled();
    expect(sendOffer).toHaveBeenCalledTimes(1);
  });

  it('uses the selected microphone for the initial call capture', async () => {
    getUserMedia.mockResolvedValueOnce(stream(track('audio')));
    await session.connect({ inputDeviceId: 'chosen-microphone' });
    expect(getUserMedia).toHaveBeenCalledWith({ audio: {
      echoCancellation: true, noiseSuppression: true, deviceId: { exact: 'chosen-microphone' },
    } });
  });

  it('does not send an offer or retain a capture after leaving during initial permission', async () => {
    const capture = deferred<MediaStream>();
    getUserMedia.mockReturnValueOnce(capture.promise);
    const connecting = session.connect();
    session.disconnect();
    const microphone = track('audio');
    capture.resolve(stream(microphone));
    await connecting;
    expect(microphone.stop).toHaveBeenCalled();
    expect(sendOffer).not.toHaveBeenCalled();
    expect(session.trackPublications.size).toBe(0);
  });

  for (const media of sources) {
    describe(media.name, () => {
      it('rejects a capture that ended before publication without losing the working device', async () => {
        await session.connect({ microphoneEnabled: false });
        const working = track(media.kind);
        const ended = track(media.kind);
        ended.readyState = 'ended';
        media.capture.mockResolvedValueOnce(stream(working)).mockResolvedValueOnce(stream(ended));
        await media.set(true);
        await expect(media.set(true)).rejects.toThrow('stopped before it could be published');
        expect(pc.transceivers[media.index].sender.track).toBe(working);
        expect(working.stop).not.toHaveBeenCalled();
        expect(ended.stop).toHaveBeenCalledOnce();
      });

      it('restores the old sender if the acquired device ends during replacement', async () => {
        await session.connect({ microphoneEnabled: false });
        const working = track(media.kind);
        const ended = track(media.kind);
        media.capture.mockResolvedValueOnce(stream(working)).mockResolvedValueOnce(stream(ended));
        await media.set(true);
        const sender = pc.transceivers[media.index].sender;
        sender.replaceTrack.mockImplementationOnce(async (value: MediaStreamTrack | null) => {
          sender.track = value;
          ended.readyState = 'ended';
        });
        await expect(media.set(true)).rejects.toThrow('stopped before it could be published');
        expect(sender.track).toBe(working);
        expect(working.stop).not.toHaveBeenCalled();
      });

      if (media.source !== TrackSource.ScreenShare) {
        it('removes an externally ended capture, surfaces recovery, and clears the error after retry', async () => {
          await session.connect({ microphoneEnabled: false });
          const captured = track(media.kind);
          const replacement = track(media.kind);
          media.capture.mockResolvedValueOnce(stream(captured)).mockResolvedValueOnce(stream(replacement));
          await media.set(true);
          const changed = vi.fn();
          session.onLocalTrackChanged = changed;
          captured.readyState = 'ended';
          captured.onended?.();
          expect(session.trackPublications.has(media.source)).toBe(false);
          expect(session.localMediaError).toContain('capture stopped unexpectedly');
          expect(session.wasMediaInterrupted(media.source)).toBe(true);
          expect(changed).toHaveBeenCalled();
          await vi.waitFor(() => expect(pc.transceivers[media.index].sender.track).toBeNull());
          await media.set(true);
          expect(session.trackPublications.get(media.source)?.track?.mediaStreamTrack).toBe(replacement);
          expect(session.localMediaError).toBeNull();
          expect(session.wasMediaInterrupted(media.source)).toBe(false);
          // A queued old native event must not unpublish the replacement.
          captured.onended?.();
          expect(pc.transceivers[media.index].sender.track).toBe(replacement);
        });

        it('does not detach a new device when the old source ends during a pending switch', async () => {
          await session.connect({ microphoneEnabled: false });
          const old = track(media.kind);
          const replacement = track(media.kind);
          media.capture.mockResolvedValueOnce(stream(old)).mockResolvedValueOnce(stream(replacement));
          await media.set(true);
          const gate = deferred<void>();
          const sender = pc.transceivers[media.index].sender;
          sender.replaceTrack.mockImplementationOnce(async (value: MediaStreamTrack | null) => {
            await gate.promise;
            sender.track = value;
          });
          const switching = media.set(true);
          await vi.waitFor(() => expect(sender.replaceTrack).toHaveBeenCalledWith(replacement));
          old.readyState = 'ended';
          old.onended?.();
          gate.resolve(undefined);
          await switching;
          await Promise.resolve();
          expect(sender.track).toBe(replacement);
          expect(replacement.stop).not.toHaveBeenCalled();
          expect(session.localMediaError).toBeNull();
          expect(session.wasMediaInterrupted(media.source)).toBe(false);
        });
      }

      it('stops capture immediately when muted during a pending sender replacement', async () => {
        await session.connect({ microphoneEnabled: false });
        const captured = track(media.kind);
        media.capture.mockResolvedValueOnce(stream(captured));
        const gate = deferred<void>();
        const sender = pc.transceivers[media.index].sender;
        sender.replaceTrack.mockImplementationOnce(async (value: MediaStreamTrack | null) => {
          await gate.promise;
          sender.track = value;
        });
        const enabling = media.set(true);
        await vi.waitFor(() => expect(sender.replaceTrack).toHaveBeenCalledWith(captured));
        const disabling = media.set(false);
        expect(captured.stop).toHaveBeenCalled();
        gate.resolve(undefined);
        await Promise.all([enabling, disabling]);
        expect(sender.track).toBeNull();
        expect(session.trackPublications.has(media.source)).toBe(false);
      });

      it('restores the working publication when a superseding capture fails', async () => {
        await session.connect({ microphoneEnabled: false });
        const working = track(media.kind);
        const provisional = track(media.kind);
        media.capture.mockResolvedValueOnce(stream(working)).mockResolvedValueOnce(stream(provisional));
        await media.set(true);
        const gate = deferred<void>();
        const sender = pc.transceivers[media.index].sender;
        sender.replaceTrack.mockImplementationOnce(async (value: MediaStreamTrack | null) => {
          await gate.promise;
          sender.track = value;
        });
        const replacing = media.set(true);
        await vi.waitFor(() => expect(sender.replaceTrack).toHaveBeenCalledWith(provisional));
        media.capture.mockRejectedValueOnce(new Error('newest device unavailable'));
        await expect(media.set(true)).rejects.toThrow('newest device unavailable');
        gate.resolve(undefined);
        await replacing;
        expect(sender.track).toBe(working);
        expect(working.stop).not.toHaveBeenCalled();
        expect(provisional.stop).toHaveBeenCalled();
      });

      it('keeps the working device and stops a new capture when replacement fails', async () => {
        await session.connect({ microphoneEnabled: false });
        const working = track(media.kind);
        const replacement = track(media.kind);
        media.capture.mockResolvedValueOnce(stream(working)).mockResolvedValueOnce(stream(replacement));
        await media.set(true);
        pc.transceivers[media.index].sender.replaceTrack.mockRejectedValueOnce(new Error('device rejected'));
        await expect(media.set(true)).rejects.toThrow('device rejected');
        expect(working.stop).not.toHaveBeenCalled();
        expect(replacement.stop).toHaveBeenCalledTimes(1);
        expect(session.trackPublications.get(media.source)?.track?.mediaStreamTrack).toBe(working);
      });

      it('cancels a pending capture when turned off', async () => {
        await session.connect({ microphoneEnabled: false });
        const capture = deferred<MediaStream>();
        media.capture.mockReturnValueOnce(capture.promise);
        const enabling = media.set(true);
        await media.set(false);
        const captured = track(media.kind);
        capture.resolve(stream(captured));
        await enabling;
        expect(captured.stop).toHaveBeenCalledTimes(1);
        expect(pc.transceivers[media.index].sender.track).toBeNull();
        expect(session.trackPublications.has(media.source)).toBe(false);
      });

      it('releases a capture that resolves after disconnect', async () => {
        await session.connect({ microphoneEnabled: false });
        const capture = deferred<MediaStream>();
        media.capture.mockReturnValueOnce(capture.promise);
        const enabling = media.set(true);
        session.disconnect();
        const captured = track(media.kind);
        capture.resolve(stream(captured));
        await enabling;
        expect(captured.stop).toHaveBeenCalledTimes(1);
        expect(session.trackPublications.size).toBe(0);
      });

      it('keeps the newest device when permission requests resolve out of order', async () => {
        await session.connect({ microphoneEnabled: false });
        const oldCapture = deferred<MediaStream>();
        const oldTrack = track(media.kind);
        const latestTrack = track(media.kind);
        media.capture.mockReturnValueOnce(oldCapture.promise).mockResolvedValueOnce(stream(latestTrack));
        const first = media.set(true);
        await media.set(true);
        oldCapture.resolve(stream(oldTrack));
        await first;
        expect(oldTrack.stop).toHaveBeenCalledTimes(1);
        expect(latestTrack.stop).not.toHaveBeenCalled();
        expect(pc.transceivers[media.index].sender.track).toBe(latestTrack);
        expect(session.trackPublications.get(media.source)?.track?.mediaStreamTrack).toBe(latestTrack);
      });
    });
  }
});
