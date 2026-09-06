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
  const getDisplayName = useUsernameStore((s) => s.getDisplayName);
  const endRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    // Captions are only useful at the live end, and new lines arrive from
    // underneath. `block: 'nearest'` keeps the scroll inside this strip
    // rather than dragging the page to it.
    endRef.current?.scrollIntoView({ block: 'nearest' });
  }, [transcripts.length]);

  // Nothing to say yet is not the same as captions being off, but the
  // distinction has no owner here: STT readiness is a server-side condition
  // reported at startup, not per call. An empty strip would be a permanent
  // fixture on every deployment without a Whisper model, so it stays out of
  // the way until there is a line to show.
  if (transcripts.length === 0) return null;

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
