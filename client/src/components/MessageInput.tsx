/**
 * Message input component — text input with send button.
 */

import { useState, type FormEvent, type KeyboardEvent } from 'react';
import { useChannelsStore } from '@/stores/channels';

export function MessageInput() {
  const [content, setContent] = useState('');
  const { activeChannelId, wsConnected, sendMessage } = useChannelsStore();

  const handleSubmit = (e: FormEvent) => {
    e.preventDefault();
    const trimmed = content.trim();
    if (!trimmed || !activeChannelId) return;
    sendMessage(trimmed);
    setContent('');
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSubmit(e);
    }
  };

  if (!activeChannelId) return null;

  return (
    <form className="message-input" onSubmit={handleSubmit}>
      <textarea
        value={content}
        onChange={(e) => setContent(e.target.value)}
        onKeyDown={handleKeyDown}
        placeholder={wsConnected ? 'Type a message...' : 'Connecting...'}
        disabled={!wsConnected}
        rows={1}
      />
      <button type="submit" disabled={!wsConnected || !content.trim()}>
        Send
      </button>
    </form>
  );
}
