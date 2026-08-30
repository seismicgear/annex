/**
 * Live captions.
 *
 * The server transcribes call audio with whisper.cpp and sends each line to
 * every participant as an `OutgoingMessage::Transcription` frame. The client
 * never read it: the frame arrived, passed validation, matched none of the
 * branches in `handleFrame`, and was dropped. A subsystem with a binary, a
 * model path, config, a startup readiness check and a broadcast channel,
 * correct at every layer, rendering nowhere.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, cleanup } from '@testing-library/react';

vi.mock('@/lib/api', () => ({ getVisibleUsernames: vi.fn(async () => ({ usernames: {} })) }));

function line(over: Record<string, unknown> = {}) {
  return {
    channelId: 'voice-1',
    speakerPseudonym: 'psn-aaaaaaaaaaaaaaaaaaaa',
    text: 'the deploy is going out at four',
    at: 1_000,
    ...over,
  };
}

async function renderCaptions(transcripts: unknown[], usernames: Record<string, string> = {}) {
  vi.resetModules();
  const { useVoiceStore } = await import('@/stores/voice');
  const { useUsernameStore } = await import('@/stores/usernames');
  const { VoiceCaptions } = await import('./VoiceCaptions');

  useVoiceStore.setState({ transcripts: transcripts as never });
  useUsernameStore.setState({ cache: usernames });
  render(<VoiceCaptions />);
}

describe('VoiceCaptions', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // jsdom implements no scrolling. Same stub the MessageView suites use.
    Element.prototype.scrollIntoView = vi.fn();
  });
  afterEach(() => cleanup());

  it('renders what was said, and who said it', async () => {
    await renderCaptions([line()], { 'psn-aaaaaaaaaaaaaaaaaaaa': 'Ada' });

    expect(screen.getByText('Ada')).toBeInTheDocument();
    expect(screen.getByText('the deploy is going out at four')).toBeInTheDocument();
  });

  it('keeps the pseudonym for a speaker who has not granted a name', async () => {
    // Not "Participant". In a call the whole use of a caption is telling two
    // unnamed speakers apart, and one shared label makes that impossible.
    await renderCaptions([
      line({ speakerPseudonym: 'psn-aaaaaaaaaaaaaaaaaaaa', text: 'first' }),
      line({ speakerPseudonym: 'psn-bbbbbbbbbbbbbbbbbbbb', text: 'second', at: 2_000 }),
    ]);

    const speakers = [...document.querySelectorAll('.voice-caption-speaker')].map(
      (el) => el.textContent,
    );
    expect(new Set(speakers).size).toBe(2);
    expect(speakers.every((s) => s !== 'Participant')).toBe(true);
  });

  it('stays out of the way when there is nothing to caption', async () => {
    // An empty strip would be a permanent fixture on every deployment that
    // ships without a Whisper model.
    await renderCaptions([]);

    expect(document.querySelector('.voice-captions')).toBeNull();
  });

  it('renders lines oldest first', async () => {
    await renderCaptions([
      line({ text: 'first', at: 1_000 }),
      line({ text: 'second', at: 2_000 }),
    ]);

    const texts = [...document.querySelectorAll('.voice-caption-text')].map((el) => el.textContent);
    expect(texts).toEqual(['first', 'second']);
  });

  it('does not collapse two identical utterances into one row', async () => {
    // Saying the same thing twice is ordinary speech. A key built from the
    // text alone would drop the repeat.
    await renderCaptions([
      line({ text: 'yes', at: 1_000 }),
      line({ text: 'yes', at: 2_000 }),
    ]);

    expect(document.querySelectorAll('.voice-caption')).toHaveLength(2);
  });
});
