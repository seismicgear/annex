/**
 * Live captions for the call in progress.
 *
 * The server has produced these all along: `whisper.cpp` transcribes call
 * audio, `OutgoingMessage::Transcription` carries each line to every
 * participant over the WebSocket, and startup reports whether STT is ready.
 * Nothing in the client read the frame. It arrived, passed validation,
 * matched none of the branches in `handleFrame`, and was dropped — a
 * subsystem correct at every layer, rendering nowhere.
 *
 * Speakers are named through the username cache, which only holds people who
 * granted this user visibility. Everyone else keeps their pseudonym rather
 * than becoming "Participant": in a call, telling two unnamed speakers apart
 * is most of what a caption is for.
 *
 * ## Silence has two causes and they do not look the same
 *
 * Nobody speaking, and no transcriber installed, both produced an empty strip.
 * A user on a server with no GGML model saw exactly what a user in a quiet
 * call saw — which is CLAUDE.md defect class 1, a failure rendered as an
 * ordinary result, and the operator had no way to learn it from the app either.
 *
 * An earlier version of this comment said "the distinction has no owner here:
 * STT readiness is a server-side condition reported at startup, not per call".
 * That was true of the component and not of the app: `/api/voice/config-status`
 * reports `stt_ready`, `loadVoiceConfig` fetches it, and it has been sitting in
 * `useVoiceStore` the whole time. The owner existed; this file was not asking.
 */

import { useEffect, useRef } from 'react';
import { useUsernameStore } from '@/stores/usernames';
import { useVoiceStore } from '@/stores/voice';

function shortPseudonym(id: string): string {
  if (!id) return 'Someone';
  return id.length > 14 ? `${id.slice(0, 14)}…` : id;
}

export function VoiceCaptions() {
  const transcripts = useVoiceStore((s) => s.transcripts);
  const voiceConfig = useVoiceStore((s) => s.voiceConfig);
  const getDisplayName = useUsernameStore((s) => s.getDisplayName);
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    // Captions are only useful at the live end, and new lines arrive from
    // underneath. `block: 'nearest'` keeps the scroll inside this strip
    // rather than dragging the page to it.
    endRef.current?.scrollIntoView({ block: 'nearest' });
  }, [transcripts.length]);

  // `stt_ready` is optional on the wire: a server that predates the field
  // sends nothing, and `undefined` means "unknown", not "broken". Only an
  // explicit `false` is a statement, and only an explicit statement earns a
  // permanent strip on screen.
  const sttUnavailable = voiceConfig?.stt_ready === false;
  // The server knows which file is missing; prefer its sentence over a
  // generic one. `stt_detail` is absent on servers that predate it.
  const sttDetail = voiceConfig?.stt_detail?.trim();

  if (transcripts.length === 0) {
    if (!sttUnavailable) return null;
    return (
      <div className="voice-captions voice-captions-unavailable" aria-label="Live captions">
        <p className="voice-caption-notice" role="status">
          {sttDetail ? (
            <>Live captions are unavailable. {sttDetail}</>
          ) : (
            <>
              Live captions are unavailable: this server has no speech-to-text model
              installed. An operator can add one with <code>scripts/setup-stt.sh</code>.
            </>
          )}
        </p>
      </div>
    );
  }

  return (
    <div className="voice-captions" aria-label="Live captions">
      <ul className="voice-caption-list">
        {transcripts.map((line) => (
          <li key={`${line.at}-${line.speakerPseudonym}-${line.text}`} className="voice-caption">
            <span className="voice-caption-speaker">
              {getDisplayName(line.speakerPseudonym) ?? shortPseudonym(line.speakerPseudonym)}
            </span>
            <span className="voice-caption-text">{line.text}</span>
          </li>
        ))}
      </ul>
      <div ref={endRef} />
    </div>
  );
}
