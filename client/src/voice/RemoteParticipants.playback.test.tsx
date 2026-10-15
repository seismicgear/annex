import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { WebRtcSession } from '@/lib/webrtc';
import { ParticipantGrid } from './RemoteParticipants';

vi.mock('@/stores/voice', () => ({
  useVoiceStore: (select: (state: unknown) => unknown) => select({
    connectedChannelId: 'channel', participantsByChannel: { channel: ['me', 'peer'] },
  }),
}));
vi.mock('@/stores/usernames', () => ({
  useUsernameStore: (select: (state: unknown) => unknown) => select({ getDisplayName: () => undefined }),
}));

class Track extends EventTarget {
  readonly id = 'video-peer';
  readyState = 'live';
  muted = false;
}
class Stream {
  constructor(readonly tracks: MediaStreamTrack[]) {}
}
function publication(track: Track) {
  return { id: track.id, sender: 'peer', track: track as unknown as MediaStreamTrack, stream: {} as MediaStream };
}
function room(track: Track): WebRtcSession {
  return {
    identity: 'me', isSpeaking: false, trackPublications: new Map(),
    remoteVideoTracks: [publication(track)], remoteAudioTracks: [],
  } as unknown as WebRtcSession;
}
function video(container: HTMLElement): HTMLVideoElement {
  return container.querySelector('video')!;
}

beforeEach(() => {
  vi.stubGlobal('MediaStream', Stream);
  vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue();
});
afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

describe('resumed remote video playback', () => {
  it('reattaches and plays a new publication even when the receiver reuses the same track object', async () => {
    const track = new Track();
    const session = room(track);
    const view = render(<ParticipantGrid session={session} />);
    const element = video(view.container);
    const firstStream = element.srcObject;
    await waitFor(() => expect(element.play).toHaveBeenCalledTimes(1));

    session.remoteVideoTracks = [publication(track)];
    view.rerender(<ParticipantGrid session={session} />);

    expect(video(view.container)).toBe(element);
    expect(element.srcObject).not.toBe(firstStream);
    expect((element.srcObject as unknown as Stream).tracks).toEqual([track]);
    await waitFor(() => expect(element.play).toHaveBeenCalledTimes(2));
  });

  it('rebinds a replacement object with the same publication ID and ignores the retired object unmuting', async () => {
    const first = new Track();
    const session = room(first);
    const view = render(<ParticipantGrid session={session} />);
    const second = new Track();
    session.remoteVideoTracks = [publication(second)];
    view.rerender(<ParticipantGrid session={session} />);
    const element = video(view.container);
    const replacementStream = element.srcObject;

    act(() => first.dispatchEvent(new Event('unmute')));
    expect(element.srcObject).toBe(replacementStream);
    expect((replacementStream as unknown as Stream).tracks).toEqual([second]);
    await waitFor(() => expect(element.play).toHaveBeenCalledTimes(2));
  });

  it('resumes the mounted receiver after an unmute without requiring a React track replacement', async () => {
    const track = new Track();
    const view = render(<ParticipantGrid session={room(track)} />);
    const element = video(view.container);
    const firstStream = element.srcObject;
    act(() => track.dispatchEvent(new Event('unmute')));
    expect(element.srcObject).not.toBe(firstStream);
    await waitFor(() => expect(element.play).toHaveBeenCalledTimes(2));
  });

  it('offers a real play retry when the browser rejects playback', async () => {
    vi.mocked(HTMLMediaElement.prototype.play).mockRejectedValueOnce(new DOMException('', 'NotAllowedError'));
    const view = render(<ParticipantGrid session={room(new Track())} />);
    fireEvent.click(await screen.findByRole('button', { name: 'Resume video' }));
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Resume video' })).not.toBeInTheDocument());
    expect(video(view.container).play).toHaveBeenCalledTimes(2);
  });

  it('does not surface a retired publication play rejection on its successor', async () => {
    let rejectPrevious!: (error: Error) => void;
    vi.mocked(HTMLMediaElement.prototype.play).mockImplementationOnce(() => new Promise((_resolve, reject) => { rejectPrevious = reject; }));
    const session = room(new Track());
    const view = render(<ParticipantGrid session={session} />);
    session.remoteVideoTracks = [publication(new Track())];
    view.rerender(<ParticipantGrid session={session} />);
    await act(async () => rejectPrevious(new DOMException('', 'AbortError')));
    expect(screen.queryByRole('button', { name: 'Resume video' })).not.toBeInTheDocument();
    expect(video(view.container).srcObject).not.toBeNull();
  });
});
