/**
 * Message view component — displays messages for the active channel.
 *
 * Shows message history with auto-scroll to bottom on new messages.
 * Supports loading older messages on scroll-up.
 * Renders privacy-preserving link previews for URLs.
 * Renders uploaded images inline with lightbox support.
 * Renders uploaded videos with playback controls.
 * Renders uploaded files as download links.
 *
 * For the local user's own messages, the persona display name and avatar
 * are shown (if set). Other users' messages show their granted username
 * (if available) or truncated pseudonyms.
 */

import { useEffect, useMemo, useRef, useState, useCallback, type ReactNode } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import { useServersStore } from '@/stores/servers';
import { useUsernameStore } from '@/stores/usernames';
import { LinkPreview } from '@/components/LinkPreview';
import { extractUrls } from '@/lib/link-preview';
import { getPersonasForIdentity } from '@/lib/personas';
import { resolveUrl } from '@/lib/api';
import * as api from '@/lib/api';
import type { Message, MessageEdit, Persona } from '@/types';

/** Edit window duration in milliseconds. */
const EDIT_WINDOW_MS = 60_000;

/** Matches URLs pointing to uploaded images on this server. */
const IMAGE_URL_PATTERN = /\/uploads\/chat\/images\/[a-f0-9-]+\.(jpg|jpeg|png|gif|webp)/i;

/** Matches URLs pointing to uploaded videos on this server. */
const VIDEO_URL_PATTERN = /\/uploads\/chat\/videos\/[a-f0-9-]+\.(mp4|webm|mov)/i;

/** Matches URLs pointing to uploaded files on this server. */
const FILE_URL_PATTERN = /\/uploads\/chat\/files\/[a-f0-9-]+\.\w+/i;

/** Legacy image URL pattern (pre-category-subdirectory uploads). */
const LEGACY_IMAGE_URL_PATTERN = /\/uploads\/chat\/[a-f0-9-]+\.(jpg|jpeg|png|gif|webp)/i;

/** All upload URL patterns. */
function isUploadUrl(url: string): boolean {
  return IMAGE_URL_PATTERN.test(url) || VIDEO_URL_PATTERN.test(url) || FILE_URL_PATTERN.test(url) || LEGACY_IMAGE_URL_PATTERN.test(url);
}

/** Parsed message content with text, images, videos, and file links. */
interface ParsedContent {
  text: string;
  imageUrls: string[];
  videoUrls: string[];
  fileUrls: string[];
}

/** Splits message content into text lines, image URLs, video URLs, and file URLs. */
function parseMessageContent(content: string): ParsedContent {
  const lines = content.split('\n');
  const textLines: string[] = [];
  const imageUrls: string[] = [];
  const videoUrls: string[] = [];
  const fileUrls: string[] = [];

  for (const line of lines) {
    const trimmed = line.trim();
    if (IMAGE_URL_PATTERN.test(trimmed) || LEGACY_IMAGE_URL_PATTERN.test(trimmed)) {
      imageUrls.push(trimmed);
    } else if (VIDEO_URL_PATTERN.test(trimmed)) {
      videoUrls.push(trimmed);
    } else if (FILE_URL_PATTERN.test(trimmed)) {
      fileUrls.push(trimmed);
    } else {
      textLines.push(line);
    }
  }

  return {
    text: textLines.join('\n').trim(),
    imageUrls,
    videoUrls,
    fileUrls,
  };
}

/** Extract filename from upload URL. */
function filenameFromUrl(url: string): string {
  const parts = url.split('/');
  return parts[parts.length - 1] || 'download';
}

/**
 * Parse a message timestamp that may be either:
 * - SQLite-style: "YYYY-MM-DD HH:MM:SS" (no timezone, assumed UTC)
 * - RFC3339: "2025-01-01T00:00:00Z" or "2025-01-01T00:00:00+00:00"
 *
 * Returns epoch ms, or NaN for malformed values.
 */
// exported for testing — see MessageView.test.tsx
// eslint-disable-next-line react-refresh/only-export-components
export function parseMessageTimestamp(ts: string): number {
  if (!ts) return NaN;
  // If already has a timezone indicator (Z, +, -), parse directly
  if (/[Zz]$/.test(ts) || /[+-]\d{2}:\d{2}$/.test(ts)) {
    return new Date(ts).getTime();
  }
  // If it contains 'T', it's an ISO-style string without timezone — treat as UTC
  if (ts.includes('T')) {
    return new Date(ts + 'Z').getTime();
  }
  // SQLite-style "YYYY-MM-DD HH:MM:SS" — append Z for UTC
  return new Date(ts + 'Z').getTime();
}

/** Returns whether a message is still within the edit/delete window. */
function isWithinEditWindow(createdAt: string): boolean {
  const created = parseMessageTimestamp(createdAt);
  if (isNaN(created)) return false;
  return Date.now() - created < EDIT_WINDOW_MS;
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
  const createdMs = parseMessageTimestamp(message.created_at);
  const time = isNaN(createdMs) ? '??' : new Date(createdMs).toLocaleTimeString();
  const isDeleted = !!message.deleted_at;
  const { text, imageUrls, videoUrls, fileUrls } = parseMessageContent(
    isDeleted ? '' : message.content,
  );
  const getDisplayName = useUsernameStore((s) => s.getDisplayName);
  const editMessage = useChannelsStore((s) => s.editMessage);
  const deleteMessage = useChannelsStore((s) => s.deleteMessage);
  const setReplyTo = useChannelsStore((s) => s.setReplyTo);
  const activeChannelId = useChannelsStore((s) => s.activeChannelId);
  const allMessages = useChannelsStore((s) => s.messages);

  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState(message.content);
  const [showHistory, setShowHistory] = useState(false);
  const [editHistory, setEditHistory] = useState<MessageEdit[] | null>(null);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [historyError, setHistoryError] = useState<string | null>(null);
  const [canModify, setCanModify] = useState(
    isSelf && !isDeleted && isWithinEditWindow(message.created_at),
  );
  const [editSecondsLeft, setEditSecondsLeft] = useState<number | null>(null);
  const editInputRef = useRef<HTMLTextAreaElement>(null);

  // Timer: update canModify when window expires, show countdown in last 30s
  useEffect(() => {
    if (!isSelf || isDeleted) {
      setCanModify(false);
      setEditSecondsLeft(null);
      return;
    }
    const created = parseMessageTimestamp(message.created_at);
    if (isNaN(created) || !isWithinEditWindow(message.created_at)) {
      setCanModify(false);
      setEditSecondsLeft(null);
      return;
    }
    setCanModify(true);
    const tick = () => {
      const remaining = Math.max(0, Math.ceil((EDIT_WINDOW_MS - (Date.now() - created)) / 1000));
      setEditSecondsLeft(remaining <= 30 ? remaining : null);
      if (remaining <= 0) {
        setCanModify(false);
        setEditSecondsLeft(null);
      }
    };
    tick();
    const interval = setInterval(tick, 1000);
    return () => clearInterval(interval);
  }, [isSelf, isDeleted, message.created_at]);

  // Focus edit input when editing starts and place cursor at end.
  // Read length from the DOM element (always current) to avoid adding
  // editText to deps, which would jump the cursor on every keystroke.
  useEffect(() => {
    if (editing && editInputRef.current) {
      editInputRef.current.focus();
      const len = editInputRef.current.value.length;
      editInputRef.current.setSelectionRange(len, len);
    }
  }, [editing]);

  // Extract external URLs from the text portion only (not uploaded media)
  const externalUrls = extractUrls(text).filter((u) => !isUploadUrl(u));

  // Show server username if available, then persona display name for own messages, then truncated pseudonym.
  let displayName: string;
  // True when the name shown is a raw cryptographic id rather than something a
  // person chose. Every other place the app prints one — `.pseudonym`,
  // `.member-pseudonym`, `.event-col-entity` — sets it in monospace; the
  // message header was the one that did not, so an id rendered here in a
  // proportional font at a width that depended on which hex digits it happened
  // to contain, and neighbouring bubbles sized differently for no visible
  // reason.
  let nameIsPseudonym = false;
  const cachedName = getDisplayName(message.sender_pseudonym);
  if (cachedName) {
    displayName = cachedName;
  } else if (isSelf && selfPersona?.displayName) {
    displayName = selfPersona.displayName;
  } else {
    displayName = message.sender_pseudonym.slice(0, 12) + '...';
    nameIsPseudonym = true;
  }

  const avatar = isSelf && selfPersona?.avatarUrl ? selfPersona.avatarUrl : null;

  const handleEdit = useCallback(() => {
    setEditText(message.content);
    setEditing(true);
  }, [message.content]);

  const handleEditSave = useCallback(() => {
    const trimmed = editText.trim();
    if (trimmed && trimmed !== message.content) {
      editMessage(message.message_id, trimmed);
    }
    setEditing(false);
  }, [editText, message.content, message.message_id, editMessage]);

  const handleEditCancel = useCallback(() => {
    setEditing(false);
    setEditText(message.content);
  }, [message.content]);

  const [confirmingDelete, setConfirmingDelete] = useState(false);

  const handleDelete = useCallback(() => {
    if (!confirmingDelete) {
      setConfirmingDelete(true);
      return;
    }
    deleteMessage(message.message_id);
    setConfirmingDelete(false);
  }, [message.message_id, deleteMessage, confirmingDelete]);

  /**
   * Fetch the edit trail. A failure must never be written into
   * `editHistory`: an empty array renders as "No edit history found", which
   * is a claim about the message — *this was never edited* — on a message
   * that visibly says it was. The audit trail is the one thing this panel
   * exists to be trusted about, so a dropped request says so and offers a
   * retry instead.
   */
  const loadHistory = useCallback(async () => {
    if (!activeChannelId) return;
    setHistoryLoading(true);
    setHistoryError(null);
    try {
      const edits = await api.getMessageEdits(pseudonymId, activeChannelId, message.message_id);
      setEditHistory(edits);
    } catch (err) {
      setEditHistory(null);
      setHistoryError(
        err instanceof Error ? err.message : 'the server did not answer',
      );
    } finally {
      setHistoryLoading(false);
    }
  }, [pseudonymId, activeChannelId, message.message_id]);

  const handleShowHistory = useCallback(async () => {
    if (showHistory) {
      setShowHistory(false);
      return;
    }
    setShowHistory(true);
    // Retry on reopen after a failure — `editHistory` stays null, so the
    // panel does not cache a failure as an answer.
    if (!editHistory) await loadHistory();
  }, [showHistory, editHistory, loadHistory]);

  const handleEditKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault();
        handleEditSave();
      } else if (e.key === 'Escape') {
        handleEditCancel();
      }
    },
    [handleEditSave, handleEditCancel],
  );

  return (
    <div className={`message ${isSelf ? 'self' : ''} ${isDeleted ? 'deleted' : ''} ${message.pending ? 'pending' : ''} ${message.failed ? 'failed' : ''}`}>
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
        <span
          className={nameIsPseudonym ? 'sender sender-pseudonym' : 'sender'}
          title={message.sender_pseudonym}
        >
          {displayName}
        </span>
        {message.edited_at && !isDeleted && (
          <button
            className="edited-badge"
            onClick={handleShowHistory}
            title="Show edit history"
          >
            (edited)
          </button>
        )}
        <span className="timestamp">{time}</span>
        {canModify && !editing && (
          <span className="message-actions">
            <button className="msg-action-btn edit-btn" onClick={handleEdit} title={editSecondsLeft !== null ? `Edit (${editSecondsLeft}s left)` : 'Edit message'}>
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" />
                <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" />
              </svg>
              {editSecondsLeft !== null && (
                <span className="edit-countdown">{editSecondsLeft}s</span>
              )}
            </button>
            <button
              className={`msg-action-btn delete-btn ${confirmingDelete ? 'confirming' : ''}`}
              onClick={handleDelete}
              onBlur={() => setConfirmingDelete(false)}
              title={confirmingDelete ? 'Click again to confirm deletion' : 'Delete message'}
            >
              {confirmingDelete ? (
                <span className="delete-confirm-text">Confirm?</span>
              ) : (
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                  <polyline points="3 6 5 6 21 6" />
                  <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
                </svg>
              )}
            </button>
          </span>
        )}
        {message.pending && (
          <span className="message-status pending-status">sending...</span>
        )}
        {message.failed && (
          <span className="message-status failed-status">
            failed
            <button className="msg-retry-btn" onClick={() => {
              if (message.clientRequestId) {
                useChannelsStore.getState().retryMessage(message.clientRequestId, pseudonymId);
              }
            }} title="Retry sending">retry</button>
            <button className="msg-dismiss-btn" onClick={() => {
              if (message.clientRequestId) {
                useChannelsStore.getState().dismissFailedMessage(message.clientRequestId);
              }
            }} title="Dismiss">dismiss</button>
          </span>
        )}
        {!isDeleted && !message.pending && !message.failed && (
          <button
            className="msg-action-btn reply-btn"
            onClick={() => setReplyTo(message)}
            title="Reply to this message"
          >
            <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
              <polyline points="9 14 4 9 9 4" />
              <path d="M20 20v-7a4 4 0 0 0-4-4H4" />
            </svg>
          </button>
        )}
      </div>

      {/* Reply context: show the quoted parent message */}
      {message.reply_to_message_id && !isDeleted && (() => {
        const parent = allMessages.find((m) => m.message_id === message.reply_to_message_id);
        if (!parent) return null;
        const parentResolved = getDisplayName(parent.sender_pseudonym);
        const parentName = parentResolved ?? parent.sender_pseudonym.slice(0, 12) + '...';
        return (
          <div className="reply-context">
            <span
              className={
                parentResolved ? 'reply-context-author' : 'reply-context-author reply-context-author-pseudonym'
              }
            >
              {parentName}
            </span>
            <span className="reply-context-text">{parent.content.slice(0, 100)}{parent.content.length > 100 ? '...' : ''}</span>
          </div>
        );
      })()}

      {isDeleted ? (
        <div className="message-content message-deleted-text">This message was deleted</div>
      ) : editing ? (
        <div className="message-edit-form">
          <textarea
            ref={editInputRef}
            className="message-edit-input"
            aria-label="Edit message"
            value={editText}
            onChange={(e) => setEditText(e.target.value)}
            onKeyDown={handleEditKeyDown}
            rows={2}
          />
          <div className="message-edit-actions">
            <button className="msg-edit-save" onClick={handleEditSave}>Save</button>
            <button className="msg-edit-cancel" onClick={handleEditCancel}>Cancel</button>
          </div>
        </div>
      ) : (
        <>
          {text && <div className="message-content">{text}</div>}
          {imageUrls.length > 0 && (
            <div className="message-images">
              {imageUrls.map((url) => (
                <img
                  key={url}
                  src={resolveUrl(url)}
                  alt="Uploaded image"
                  className="message-inline-image"
                  loading="lazy"
                  onClick={() => onImageClick(url)}
                />
              ))}
            </div>
          )}
          {videoUrls.length > 0 && (
            <div className="message-videos">
              {videoUrls.map((url) => (
                <video
                  key={url}
                  src={resolveUrl(url)}
                  className="message-inline-video"
                  controls
                  preload="metadata"
                  playsInline
                />
              ))}
            </div>
          )}
          {fileUrls.length > 0 && (
            <div className="message-files">
              {fileUrls.map((url) => (
                <a
                  key={url}
                  href={resolveUrl(url)}
                  className="message-file-link"
                  download
                  target="_blank"
                  rel="noopener noreferrer"
                >
                  <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                    <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
                    <polyline points="14 2 14 8 20 8" />
                  </svg>
                  <span>{filenameFromUrl(url)}</span>
                </a>
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
        </>
      )}

      {showHistory && (
        <div className="edit-history">
          <div className="edit-history-header">Edit History</div>
          {historyLoading ? (
            <div className="edit-history-loading">Loading...</div>
          ) : historyError ? (
            <div className="edit-history-error" role="alert">
              <span>Could not load edit history: {historyError}</span>
              <button onClick={loadHistory}>Retry</button>
            </div>
          ) : editHistory && editHistory.length > 0 ? (
            <div className="edit-history-list">
              {editHistory.map((edit) => (
                <div key={edit.id} className="edit-history-item">
                  <div className="edit-history-content">{edit.old_content}</div>
                  <div className="edit-history-time">
                    {(() => {
                      const ms = parseMessageTimestamp(edit.edited_at);
                      return isNaN(ms) ? '??' : new Date(ms).toLocaleString();
                    })()}
                  </div>
                </div>
              ))}
              <div className="edit-history-item edit-history-current">
                <div className="edit-history-content">{message.content}</div>
                <div className="edit-history-time">Current version</div>
              </div>
            </div>
          ) : (
            <div className="edit-history-empty">No edit history found</div>
          )}
        </div>
      )}
    </div>
  );
}

export function MessageView() {
  const identity = useIdentityStore((s) => s.identity);
  const { messages, activeChannelId, loadOlderMessages, loadingOlder, hasMoreMessages, historyLoading, historyError, typingUsers, olderError, retryOlderMessages, editError, clearEditError } = useChannelsStore();
  const selectChannel = useChannelsStore((s) => s.selectChannel);
  const loadVisibleUsernames = useUsernameStore((s) => s.loadVisibleUsernames);
  // The cache itself, not the getter: this has to re-evaluate when a fetch
  // fills in names, or the effect below would refetch on every render.
  const usernameCache = useUsernameStore((s) => s.cache);
  const bottomRef = useRef<HTMLDivElement>(null);
  const containerRef = useRef<HTMLDivElement>(null);
  const prevMessageCount = useRef(0);
  const prevScrollHeight = useRef(0);
  const [selfPersona, setSelfPersona] = useState<Persona | null>(null);
  const [lightboxUrl, setLightboxUrl] = useState<string | null>(null);
  // Subscribe to server accent color so persona reloads when user changes color
  const serverAccentColor = useServersStore((s) => s.getActiveServer()?.accentColor);

  // Load the local user's persona for display name / avatar
  useEffect(() => {
    if (!identity) return;
    getPersonasForIdentity(identity.id).then((list) => {
      setSelfPersona(list[0] ?? null);
    });
  }, [identity, serverAccentColor]);

  // Load visible usernames from server.
  //
  // Refetched when a sender appears that the cache cannot name, not just
  // once on mount. The cache was previously filled a single time per
  // identity, so anyone who joined the server, was granted username
  // visibility, or simply spoke for the first time after the chat was
  // opened stayed a raw pseudonym for the rest of the session — and the
  // longer a session ran the more of the room was anonymous.
  //
  // Keyed on the set of unnamed senders so a channel full of people who are
  // legitimately unnamed (visibility not granted) settles after one attempt
  // instead of refetching forever: the key only changes when a NEW unknown
  // pseudonym shows up.
  const unnamedKey = useMemo(() => {
    const unknown = new Set<string>();
    for (const m of messages) {
      if (m.sender_pseudonym && !usernameCache[m.sender_pseudonym]) {
        unknown.add(m.sender_pseudonym);
      }
    }
    return [...unknown].sort().join(',');
  }, [messages, usernameCache]);

  useEffect(() => {
    if (!identity?.pseudonymId) return;
    void loadVisibleUsernames(identity.pseudonymId);
  }, [identity?.pseudonymId, loadVisibleUsernames, unnamedKey]);

  // Auto-scroll to bottom on new messages; preserve scroll position on prepend
  useEffect(() => {
    const el = containerRef.current;
    if (!el) {
      prevMessageCount.current = messages.length;
      return;
    }
    if (messages.length > prevMessageCount.current) {
      const added = messages.length - prevMessageCount.current;
      // If scroll was at the top and we added messages at the top (older messages loaded),
      // preserve the user's reading position by restoring the scroll offset.
      if (prevScrollHeight.current > 0 && el.scrollTop < 10) {
        const newScrollTop = el.scrollHeight - prevScrollHeight.current;
        el.scrollTop = newScrollTop;
      } else {
        // New messages appended at bottom — auto-scroll down
        bottomRef.current?.scrollIntoView({ behavior: added > 10 ? 'auto' : 'smooth' });
      }
    }
    prevMessageCount.current = messages.length;
    prevScrollHeight.current = 0;
  }, [messages.length]);

  // Load older messages on scroll to top
  const pseudonymId = identity?.pseudonymId;
  const messageCount = messages.length;
  const handleScroll = () => {
    const el = containerRef.current;
    if (!el || !pseudonymId) return;
    if (el.scrollTop <= 1 && messageCount > 0 && !loadingOlder && hasMoreMessages) {
      // Save scroll height so the effect can restore position after prepend
      prevScrollHeight.current = el.scrollHeight;
      loadOlderMessages(pseudonymId);
    }
  };

  // Every state renders through the same shell.
  //
  // The empty, loading and error branches used to return a bare
  // `<div className="message-view empty">`, which cost two things. axe reported
  // `scrollable-region-focusable` against them — `.message-view` scrolls, and a
  // scrollable region that cannot take focus cannot be scrolled from the
  // keyboard at all. Less visibly, `role="log"` and `aria-live="polite"` only
  // existed on the populated branch, so switching to an empty channel
  // destroyed the live region and the first message to arrive created it and
  // landed in the same tick — which is exactly the case a screen reader does
  // not announce.
  const shell = (children: ReactNode, extraClass = '') => (
    <div
      className={`message-view ${extraClass}`.trim()}
      role="log"
      aria-label="Message history"
      aria-live="polite"
      tabIndex={0}
    >
      {children}
    </div>
  );

  if (!activeChannelId) {
    return shell(<p>Select a channel to start chatting</p>, 'empty');
  }

  if (historyLoading) {
    return shell(<p>Loading channel history...</p>, 'empty');
  }

  if (historyError) {
    return shell(
      <>
        <p className="error-message">{historyError}</p>
        <button
          className="primary-btn"
          onClick={() => {
            if (identity?.pseudonymId && activeChannelId) {
              selectChannel(identity.pseudonymId, activeChannelId);
            }
          }}
        >
          Retry
        </button>
      </>,
      'empty',
    );
  }

  if (messages.length === 0 && !loadingOlder) {
    return shell(<p>No messages yet — be the first to say something!</p>, 'empty');
  }

  return (
    <>
      {/* Same contract as `shell` above; this branch keeps its own element
          because it needs the scroll ref and handler. */}
      <div
        className="message-view"
        ref={containerRef}
        onScroll={handleScroll}
        role="log"
        aria-label="Message history"
        aria-live="polite"
        tabIndex={0}
      >
        {olderError && (
          <div className="scrollback-error" role="alert">
            <span>{olderError}</span>
            <button
              type="button"
              onClick={() => identity?.pseudonymId && void retryOlderMessages(identity.pseudonymId)}
            >
              Retry
            </button>
          </div>
        )}
        {messages.map((msg: Message) => (
          <MessageBubble
            key={msg.clientRequestId ?? msg.message_id}
            message={msg}
            isSelf={msg.sender_pseudonym === identity?.pseudonymId}
            pseudonymId={identity?.pseudonymId ?? ''}
            selfPersona={selfPersona}
            onImageClick={setLightboxUrl}
          />
        ))}
        {typingUsers.length > 0 && (
          <div className="typing-indicator">
            {typingUsers.length === 1
              ? `${typingUsers[0].pseudonymId.slice(0, 12)}... is typing...`
              : typingUsers.length <= 3
                ? `${typingUsers.map((u) => u.pseudonymId.slice(0, 8) + '...').join(', ')} are typing...`
                : 'Several people are typing...'}
          </div>
        )}
        <div ref={bottomRef} />
      </div>

      {editError && (
        <div className="edit-error" role="alert">
          <span>{editError}</span>
          <button type="button" onClick={clearEditError} aria-label="Dismiss">&times;</button>
        </div>
      )}

      {lightboxUrl && (
        <div className="image-lightbox" onClick={() => setLightboxUrl(null)}>
          <img src={resolveUrl(lightboxUrl)} alt="Full size" />
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
