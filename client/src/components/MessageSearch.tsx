/**
 * Message search — inline search bar for finding messages across channels.
 */

import { useState, useCallback, useRef, useEffect } from 'react';
import { useIdentityStore } from '@/stores/identity';
import { useChannelsStore } from '@/stores/channels';
import * as api from '@/lib/api';
import { parseMessageTimestamp } from '@/components/MessageView';
import type { Message } from '@/types';

export function MessageSearch() {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<Message[]>([]);
  const [searching, setSearching] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const identity = useIdentityStore((s) => s.identity);
  const activeChannelId = useChannelsStore((s) => s.activeChannelId);
  const selectChannel = useChannelsStore((s) => s.selectChannel);

  // Keyboard shortcut: Ctrl+F or Cmd+F to open search
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key === 'f') {
        e.preventDefault();
        setOpen(true);
        setTimeout(() => inputRef.current?.focus(), 50);
      }
      if (e.key === 'Escape' && open) {
        setOpen(false);
        setResults([]);
        setQuery('');
      }
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [open]);

  const handleSearch = useCallback(async () => {
    if (!query.trim() || !identity?.pseudonymId) return;
    setSearching(true);
    setError(null);
    try {
      const msgs = await api.searchMessages(
        identity.pseudonymId,
        query.trim(),
        activeChannelId ?? undefined,
        20,
      );
      setResults(msgs);
    } catch (err) {
      // A failed request is not an empty result set. Rendering it as "No
      // messages found" tells the user their search worked and the thing
      // they are looking for does not exist — which is the one conclusion
      // the server never actually reported.
      console.warn('[search] request failed:', err);
      setResults([]);
      setError('Search failed. Check your connection and try again.');
    } finally {
      setSearching(false);
    }
  }, [query, identity?.pseudonymId, activeChannelId]);

  const handleResultClick = (msg: Message) => {
    if (!identity?.pseudonymId) return;
    // Navigate to the channel containing the message
    selectChannel(identity.pseudonymId, msg.channel_id);
    setOpen(false);
    setResults([]);
    setQuery('');
  };

  if (!open) {
    return (
      <button
        className="search-toggle-btn"
        onClick={() => { setOpen(true); setTimeout(() => inputRef.current?.focus(), 50); }}
        title="Search messages (Ctrl+F)"
        aria-label="Search messages"
      >
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
          <circle cx="11" cy="11" r="8" />
          <line x1="21" y1="21" x2="16.65" y2="16.65" />
        </svg>
      </button>
    );
  }

  return (
    <div className="message-search" role="search">
      <form
        className="search-form"
        onSubmit={(e) => { e.preventDefault(); handleSearch(); }}
      >
        <input
          ref={inputRef}
          type="text"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={activeChannelId ? 'Search this channel...' : 'Search all messages...'}
          aria-label="Search messages"
        />
        <button type="submit" disabled={searching || !query.trim()}>
          {searching ? '...' : 'Search'}
        </button>
        <button type="button" onClick={() => { setOpen(false); setResults([]); setQuery(''); }} aria-label="Close search">
          &times;
        </button>
      </form>
      {results.length > 0 && (
        <div className="search-results" role="listbox" aria-label="Search results">
          {results.map((msg) => {
            const ts = parseMessageTimestamp(msg.created_at);
            const time = isNaN(ts) ? '' : new Date(ts).toLocaleString();
            return (
              <button
                key={msg.message_id}
                className="search-result-item"
                onClick={() => handleResultClick(msg)}
                role="option"
              >
                <span className="search-result-sender">{msg.sender_pseudonym.slice(0, 12)}...</span>
                <span className="search-result-content">{msg.content.slice(0, 120)}{msg.content.length > 120 ? '...' : ''}</span>
                <span className="search-result-time">{time}</span>
              </button>
            );
          })}
        </div>
      )}
      {error && !searching && (
        <div className="search-error" role="alert">
          <span>{error}</span>
          <button type="button" onClick={() => void handleSearch()}>Retry</button>
        </div>
      )}
      {!error && results.length === 0 && query.trim() && !searching && (
        <div className="search-no-results">No messages found</div>
      )}
    </div>
  );
}
