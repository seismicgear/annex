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

async function renderCaptions(
  transcripts: unknown[],
  usernames: Record<string, string> = {},
  voiceConfig: { stt_ready?: boolean; stt_detail?: string } | null = null,
) {
  vi.resetModules();
  const { useVoiceStore } = await import('@/stores/voice');
  const { useUsernameStore } = await import('@/stores/usernames');
  const { VoiceCaptions } = await import('./VoiceCaptions');

  useVoiceStore.setState({
    transcripts: transcripts as never,
    voiceConfig: voiceConfig as never,
  });
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
    // Quiet, and no statement either way about whether STT works. Nothing to
    // say, so it says nothing.
    await renderCaptions([]);

    expect(document.querySelector('.voice-captions')).toBeNull();
  });

  // ── Silence has two causes ────────────────────────────────────────────────
  //
  // Nobody speaking, and no transcriber installed, both produced an empty
  // strip — CLAUDE.md defect class 1, a failure rendered as an ordinary
  // result. The information existed the whole time:
  // `/api/voice/config-status` reports `stt_ready`, `loadVoiceConfig` fetches
  // it, and it sits in the voice store. This file simply was not asking.

  it('says captions are unavailable when the server reports no STT model', async () => {
    await renderCaptions([], {}, { stt_ready: false });

    const notice = screen.getByRole('status');
    expect(notice).toBeInTheDocument();
    expect(notice.textContent).toMatch(/unavailable/i);
    // Names what an operator has to do. A notice that says only "unavailable"
    // leaves the person who can fix it with nowhere to go.
    expect(notice.textContent).toMatch(/setup-stt\.sh/);
  });

  it('says nothing when STT readiness is unknown', async () => {
    // `stt_ready` is optional on the wire: a server that predates the field
    // sends nothing. `undefined` is "unknown", not "broken", and only an
    // explicit statement earns a permanent strip on screen.
    await renderCaptions([], {}, {});
    expect(document.querySelector('.voice-captions')).toBeNull();

    await cleanup();
    await renderCaptions([], {}, null);
    expect(document.querySelector('.voice-captions')).toBeNull();
  });

  it('names the file the server says is missing, not a generic cause', async () => {
    // `stt_detail` distinguishes four situations a bare `stt_ready: false`
    // collapses into one. The one an operator hits most is the third.
    await renderCaptions([], {}, {
      stt_ready: false,
      stt_detail:
        'The whisper.cpp binary at /opt/whisper/bin/whisper exists but is not executable. Run `chmod +x` on it, or re-run scripts/setup-stt.sh.',
    });

    const notice = screen.getByRole('status');
    expect(notice.textContent).toMatch(/not executable/);
    expect(notice.textContent).toMatch(/\/opt\/whisper\/bin\/whisper/);
  });

  it('falls back to the generic notice when the server sends no detail', async () => {
    // Servers that predate `stt_detail` send only the boolean.
    await renderCaptions([], {}, { stt_ready: false });
    expect(screen.getByRole('status').textContent).toMatch(/setup-stt\.sh/);
  });

  it('shows the lines, not the notice, once transcription is working', async () => {
    // The notice must not survive into a working call — a server can become
    // ready, and a stale "unavailable" banner over live captions is worse than
    // the empty strip it replaced.
    await renderCaptions([line()], {}, { stt_ready: true });

    expect(screen.queryByRole('status')).toBeNull();
    expect(screen.getByText('the deploy is going out at four')).toBeInTheDocument();
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
