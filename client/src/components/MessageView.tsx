/**
 * Message view component — displays messages for the active channel.
 *
 * Shows message history with auto-scroll to bottom on new messages.
 * Supports loading older messages on scroll-up.
 * Renders privacy-preserving link previews for URLs.
 */

import { useEffect, useRef } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import { LinkPreview } from '@/components/LinkPreview';
import { extractUrls } from '@/lib/link-preview';
import type { Message } from '@/types';

function MessageBubble({
  message,
  isSelf,
  pseudonymId,
}: {
  message: Message;
  isSelf: boolean;
  pseudonymId: string;
}) {
  const time = new Date(message.created_at).toLocaleTimeString();
  const urls = extractUrls(message.content);
  return (
    <div className={`message ${isSelf ? 'self' : ''}`}>
      <div className="message-header">
        <span className="sender">{message.sender_pseudonym.slice(0, 12)}...</span>
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
        />
      ))}
      <div ref={bottomRef} />
    </div>
  );
}
