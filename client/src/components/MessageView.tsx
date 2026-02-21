/**
 * Message view component — displays messages for the active channel.
 *
 * Shows message history with auto-scroll to bottom on new messages.
 * Supports loading older messages on scroll-up.
 * Renders privacy-preserving link previews for URLs.
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

function MessageBubble({
  message,
  isSelf,
  pseudonymId,
  selfPersona,
}: {
  message: Message;
  isSelf: boolean;
  pseudonymId: string;
  selfPersona: Persona | null;
}) {
  const time = new Date(message.created_at).toLocaleTimeString();
  const urls = extractUrls(message.content);

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
      <div className="message-content">{message.content}</div>
      {urls.length > 0 && (
        <div className="message-previews">
          {urls.slice(0, 3).map((url) => (
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
    <div className="message-view" ref={containerRef} onScroll={handleScroll}>
      {messages.map((msg: Message) => (
        <MessageBubble
          key={msg.message_id}
          message={msg}
          isSelf={msg.sender_pseudonym === identity?.pseudonymId}
          pseudonymId={identity?.pseudonymId ?? ''}
          selfPersona={selfPersona}
        />
      ))}
      <div ref={bottomRef} />
    </div>
  );
}
