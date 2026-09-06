import { describe, it, expect } from 'vitest';
import { senderFromTrackId } from './webrtc';

/**
 * The track-id contract between the SFU and this client.
 *
 * `crates/annex-voice/src/service.rs::make_outbound_tracks` names every
 * outbound track `audio-<pseudonym>` / `video-<pseudonym>` for the peer whose
 * media it carries. That reaches the browser as the MSID and is the only
 * per-track attribution available — before per-sender tracks existed, the
 * client had a bag of anonymous tracks and every remote tile rendered the
 * literal string "Participant".
 *
 * The Rust side pins the format in `track_ids_name_the_sender`; this pins the
 * parse. If the two ever disagree, tiles silently lose their names rather than
 * failing, so both ends are asserted explicitly.
 */
describe('senderFromTrackId', () => {
  it('reads the sender out of an audio track id', () => {
    expect(senderFromTrackId('audio-a1b2c3d4e5f6')).toBe('a1b2c3d4e5f6');
  });

  it('reads the sender out of a video track id', () => {
    expect(senderFromTrackId('video-a1b2c3d4e5f6')).toBe('a1b2c3d4e5f6');
  });

  it('handles a full-length pseudonym', () => {
    const pseudonym = 'f'.repeat(64);
    expect(senderFromTrackId(`audio-${pseudonym}`)).toBe(pseudonym);
  });

  // Pseudonyms are hex today, but the parse must not depend on that — a
  // pseudonym format change should not silently unname every tile.
  it('does not assume the pseudonym is hex', () => {
    expect(senderFromTrackId('audio-user_with-punctuation.123')).toBe(
      'user_with-punctuation.123',
    );
  });

  // The agent (TTS) track is not attributable to a person. Returning null
  // gives an unnamed tile rather than a wrong name.
  it('returns null for a track it cannot attribute', () => {
    expect(senderFromTrackId('agent-mix-chan-1')).toBeNull();
    expect(senderFromTrackId('')).toBeNull();
    expect(senderFromTrackId('audio-')).toBeNull();
    expect(senderFromTrackId('somethingelse')).toBeNull();
  });

  it('keeps a hyphenated remainder intact rather than splitting on every dash', () => {
    expect(senderFromTrackId('video-abc-def-ghi')).toBe('abc-def-ghi');
  });
});
