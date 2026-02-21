/**
 * Message view component — displays messages for the active channel.
 *
 * Shows message history with auto-scroll to bottom on new messages.
 * Supports loading older messages on scroll-up.
 * Renders privacy-preserving link previews for URLs.
 * Renders uploaded images inline with lightbox support.
 *
 * For the local user's own messages, the persona display name and avatar
 * are shown (if set). Other users' messages show truncated pseudonyms
 * because display names are client-local and never sent to the server.
 */

import { useEffect, useRef, useState } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import { LinkPreview } from '@/components/LinkPreview';
import { extractUrls } from '@/lib/link-preview';
import { getPersonasForIdentity } from '@/lib/personas';
import type { Message, Persona } from '@/types';

/** Matches URLs pointing to uploaded images on this server. */
const UPLOAD_URL_PATTERN = /\/uploads\/chat\/[a-f0-9-]+\.(jpg|jpeg|png|gif|webp)/i;

/** Splits message content into text lines and inline image URLs. */
function parseMessageContent(content: string): { text: string; imageUrls: string[] } {
  const lines = content.split('\n');
  const textLines: string[] = [];
  const imageUrls: string[] = [];

  for (const line of lines) {
    const trimmed = line.trim();
    if (UPLOAD_URL_PATTERN.test(trimmed)) {
      imageUrls.push(trimmed);
    } else {
      textLines.push(line);
    }
  }

  return {
    text: textLines.join('\n').trim(),
    imageUrls,
  };
}

function MessageBubble({
  message,
  isSelf,
  pseudonymId,
  selfPersona,
  onImageClick,
}: {
  message: Message;
  isSelf: boolean;
  pseudonymId: string;
  selfPersona: Persona | null;
  onImageClick: (url: string) => void;
}) {
  const time = new Date(message.created_at).toLocaleTimeString();
  const { text, imageUrls } = parseMessageContent(message.content);

  // Extract external URLs from the text portion only (not uploaded images)
  const externalUrls = extractUrls(text).filter(
    (u) => !UPLOAD_URL_PATTERN.test(u),
  );

  // Show persona display name for own messages; truncated pseudonym for others.
  const displayName = isSelf && selfPersona?.displayName
    ? selfPersona.displayName
    : message.sender_pseudonym.slice(0, 12) + '...';

  const avatar = isSelf && selfPersona?.avatarUrl ? selfPersona.avatarUrl : null;

  return (
    <div className={`message ${isSelf ? 'self' : ''}`}>
      <div className="message-header">
        {avatar ? (
          <img className="message-avatar" src={avatar} alt="" />
        ) : (
          <span
            className="message-avatar-placeholder"
            style={isSelf && selfPersona?.accentColor ? { background: selfPersona.accentColor } : undefined}
          >
            {displayName.charAt(0).toUpperCase()}
          </span>
        )}
        <span className="sender" title={message.sender_pseudonym}>{displayName}</span>
        <span className="timestamp">{time}</span>
      </div>
      {text && <div className="message-content">{text}</div>}
      {imageUrls.length > 0 && (
        <div className="message-images">
          {imageUrls.map((url) => (
            <img
              key={url}
              src={url}
              alt="Uploaded image"
              className="message-inline-image"
              loading="lazy"
              onClick={() => onImageClick(url)}
            />
          ))}
        </div>
      )}
      {externalUrls.length > 0 && (
        <div className="message-previews">
          {externalUrls.slice(0, 3).map((url) => (
            <LinkPreview key={url} url={url} pseudonymId={pseudonymId} />
          ))}
        </div>
      )}
    </div>
  );
}

export function MessageView() {
  const identity = useIdentityStore((s) => s.identity);
  const { messages, activeChannelId, loadOlderMessages } = useChannelsStore();
  const bottomRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const prevMessageCount = useRef(0);
  const [selfPersona, setSelfPersona] = useState<Persona | null>(null);
  const [lightboxUrl, setLightboxUrl] = useState<string | null>(null);

  // Load the local user's persona for display name / avatar
  useEffect(() => {
    if (!identity) return;
    getPersonasForIdentity(identity.id).then((list) => {
      setSelfPersona(list[0] ?? null);
    });
  }, [identity]);

  // Auto-scroll to bottom on new messages
  useEffect(() => {
    if (messages.length > prevMessageCount.current) {
      bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
    }
    prevMessageCount.current = messages.length;
  }, [messages.length]);

  // Load older messages on scroll to top
  const pseudonymId = identity?.pseudonymId;
  const messageCount = messages.length;
  const handleScroll = () => {
    const el = containerRef.current;
    if (!el || !pseudonymId) return;
    if (el.scrollTop === 0 && messageCount > 0) {
      loadOlderMessages(pseudonymId);
    }
  };

  if (!activeChannelId) {
    return (
      <div className="message-view empty">
        <p>Select a channel to start chatting</p>
      </div>
    );
  }

  return (
    <>
      <div className="message-view" ref={containerRef} onScroll={handleScroll}>
        {messages.map((msg: Message) => (
          <MessageBubble
            key={msg.message_id}
            message={msg}
            isSelf={msg.sender_pseudonym === identity?.pseudonymId}
            pseudonymId={identity?.pseudonymId ?? ''}
            selfPersona={selfPersona}
            onImageClick={setLightboxUrl}
          />
        ))}
        <div ref={bottomRef} />
      </div>

      {lightboxUrl && (
        <div className="image-lightbox" onClick={() => setLightboxUrl(null)}>
          <img src={lightboxUrl} alt="Full size" />
          <button
            className="lightbox-close"
            onClick={() => setLightboxUrl(null)}
          >
            x
          </button>
        </div>
      )}
    </>
  );
}
