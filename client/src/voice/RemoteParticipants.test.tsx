/**
 * Naming the people in a call.
 *
 * Every remote tile used to render the literal string "Participant" — a call
 * of four looked like four identical anonymous boxes. The SFU had always keyed
 * peers by pseudonym; only the count was exposed, so the client genuinely had
 * nothing to show. `voice/status` now returns the roster.
 *
 * It cannot name every tile, though, and these tests pin where the line is.
 * `fan_out_rtp` writes every sender's RTP into a single outbound track per
 * receiver, so a tile can only be attributed to a person when there is exactly
 * one other person in the call. With two or more remote senders, naming a tile
 * would be a guess — wrong at least half the time — so the tiles stay unnamed
 * and the roster is listed instead.
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import type { WebRtcSession } from '@/lib/webrtc';
import { ParticipantGrid } from './RemoteParticipants';

const voiceState = {
  connectedChannelId: 'chan-1' as string | null,
  participantsByChannel: {} as Record<string, string[]> | undefined,
};
const usernames = { getDisplayName: (id: string): string | undefined => void id };

vi.mock('@/stores/voice', () => ({
  useVoiceStore: (sel: (s: typeof voiceState) => unknown) => sel(voiceState),
}));
vi.mock('@/stores/usernames', () => ({
  useUsernameStore: (sel: (s: typeof usernames) => unknown) => sel(usernames),
}));

/**
 * A session with `remoteCount` live remote video tracks and `audioCount`
 * inbound audio tracks.
 *
 * `audioCount` defaults to 3 on purpose: that is what a real connection looks
 * like even when nobody else is in the channel. The SFU attaches an audio mix,
 * a video slot and the agent TTS track to every peer connection at join, so
 * inbound track count says nothing about how many people are present.
 */
function session(identity: string, remoteCount: number, audioCount = 3): WebRtcSession {
  return {
    identity,
    isSpeaking: false,
    trackPublications: new Map(),
    remoteVideoTracks: Array.from({ length: remoteCount }, (_, i) => ({
      id: `v${i}`,
      track: { readyState: 'live', muted: false } as unknown as MediaStreamTrack,
      stream: {} as MediaStream,
    })),
    remoteAudioTracks: Array.from({ length: audioCount }, (_, i) => ({
      id: `a${i}`,
      track: {} as MediaStreamTrack,
      stream: {} as MediaStream,
    })),
  } as unknown as WebRtcSession;
}

// jsdom implements no WebRTC media types; `VideoTile` constructs a MediaStream
// to attach the track to a <video>. The grid's naming logic is what is under
// test here, so a constructor stub is enough.
beforeEach(() => {
  vi.stubGlobal(
    'MediaStream',
    class {
      constructor(public tracks: unknown[] = []) {}
    },
  );
  voiceState.connectedChannelId = 'chan-1';
  voiceState.participantsByChannel = {};
  usernames.getDisplayName = () => undefined;
});

describe('ParticipantGrid naming', () => {
  it('names the remote tile when exactly one other person is in the call', () => {
    voiceState.participantsByChannel = { 'chan-1': ['me', 'a1b2c3d4e5f6'] };
    render(<ParticipantGrid session={session('me', 1)} />);

    expect(screen.getByText('a1b2c3d4e5f6')).toBeInTheDocument();
    expect(screen.queryByText('Participant')).not.toBeInTheDocument();
  });

  it('prefers a resolved username over the raw pseudonym', () => {
    voiceState.participantsByChannel = { 'chan-1': ['me', 'a1b2c3d4e5f6'] };
    usernames.getDisplayName = (id) => (id === 'a1b2c3d4e5f6' ? 'Ada' : undefined);
    render(<ParticipantGrid session={session('me', 1)} />);

    expect(screen.getByText('Ada')).toBeInTheDocument();
  });

  it('truncates a long pseudonym rather than overflowing the tile', () => {
    const long = 'f'.repeat(64);
    voiceState.participantsByChannel = { 'chan-1': ['me', long] };
    render(<ParticipantGrid session={session('me', 1)} />);

    expect(screen.getByText(`${'f'.repeat(14)}…`)).toBeInTheDocument();
  });

  // The important negative case: with two remote senders the SFU collapses
  // them onto one track, so there is no way to know which tile is whom.
  it('leaves tiles unnamed and lists the roster when more than one other is present', () => {
    voiceState.participantsByChannel = { 'chan-1': ['me', 'alice-id', 'bob-id'] };
    usernames.getDisplayName = (id) =>
      ({ 'alice-id': 'Alice', 'bob-id': 'Bob' })[id];
    render(<ParticipantGrid session={session('me', 2)} />);

    expect(screen.getAllByText('Participant')).toHaveLength(2);
    expect(screen.getByText('In this call: Alice, Bob')).toBeInTheDocument();
  });

  it('does not list a roster when it could name the tile', () => {
    voiceState.participantsByChannel = { 'chan-1': ['me', 'alice-id'] };
    render(<ParticipantGrid session={session('me', 1)} />);

    expect(screen.queryByText(/In this call:/)).not.toBeInTheDocument();
  });

  it('excludes you from the roster', () => {
    voiceState.participantsByChannel = { 'chan-1': ['me'] };
    render(<ParticipantGrid session={session('me', 0)} />);

    expect(screen.queryByText(/In this call:/)).not.toBeInTheDocument();
    expect(screen.getByText('You')).toBeInTheDocument();
  });

  // Tiles used to be counted from `remoteAudioTracks.length`, which is never
  // zero on a live connection — so sitting alone in a voice channel rendered a
  // tile captioned "Participant" and looked exactly like somebody else being
  // there.
  it('shows no remote tile when you are alone in the call', () => {
    voiceState.participantsByChannel = { 'chan-1': ['me'] };
    render(<ParticipantGrid session={session('me', 0)} />);

    expect(screen.queryByText('Participant')).not.toBeInTheDocument();
    expect(screen.getAllByText('You')).toHaveLength(1);
  });

  it('shows exactly one remote tile for one other person, whatever the track count', () => {
    voiceState.participantsByChannel = { 'chan-1': ['me', 'alice-id'] };
    // Five inbound audio tracks, one other person. The roster decides.
    render(<ParticipantGrid session={session('me', 0, 5)} />);

    expect(screen.getAllByText('alice-id')).toHaveLength(1);
  });

  // A store rehydrated from a build that predates the roster has no such key.
  // That must degrade to unnamed tiles, not take down the whole call view.
  it('survives a store with no roster at all', () => {
    voiceState.participantsByChannel = undefined;
    expect(() => render(<ParticipantGrid session={session('me', 1)} />)).not.toThrow();
    expect(screen.getByText('Participant')).toBeInTheDocument();
  });

  it('survives not being connected to a channel', () => {
    voiceState.connectedChannelId = null;
    expect(() => render(<ParticipantGrid session={session('me', 1)} />)).not.toThrow();
  });
});
