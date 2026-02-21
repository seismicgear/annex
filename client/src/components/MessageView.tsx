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

import { useEffect, useRef, useState, useCallback } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import { useServersStore } from '@/stores/servers';
import { useUsernameStore } from '@/stores/usernames';
import { LinkPreview } from '@/components/LinkPreview';
import { extractUrls } from '@/lib/link-preview';
import { getPersonasForIdentity } from '@/lib/personas';
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

/** Returns whether a message is still within the edit/delete window. */
function isWithinEditWindow(createdAt: string): boolean {
  const created = new Date(createdAt + 'Z').getTime();
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
  const time = new Date(message.created_at).toLocaleTimeString();
  const isDeleted = !!message.deleted_at;
  const { text, imageUrls, videoUrls, fileUrls } = parseMessageContent(
    isDeleted ? '' : message.content,
  );
  const getDisplayName = useUsernameStore((s) => s.getDisplayName);
  const editMessage = useChannelsStore((s) => s.editMessage);
  const deleteMessage = useChannelsStore((s) => s.deleteMessage);
  const activeChannelId = useChannelsStore((s) => s.activeChannelId);

  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState(message.content);
  const [showHistory, setShowHistory] = useState(false);
  const [editHistory, setEditHistory] = useState<MessageEdit[] | null>(null);
  const [historyLoading, setHistoryLoading] = useState(false);
  const [canModify, setCanModify] = useState(
    isSelf && !isDeleted && isWithinEditWindow(message.created_at),
  );
  const editInputRef = useRef<HTMLTextAreaElement>(null);

  // Timer: update canModify when window expires
  useEffect(() => {
    if (!isSelf || isDeleted) {
      setCanModify(false);
      return;
    }
    if (!isWithinEditWindow(message.created_at)) {
      setCanModify(false);
      return;
    }
    setCanModify(true);
    const created = new Date(message.created_at + 'Z').getTime();
    const remaining = EDIT_WINDOW_MS - (Date.now() - created);
    const timer = setTimeout(() => setCanModify(false), remaining);
    return () => clearTimeout(timer);
  }, [isSelf, isDeleted, message.created_at]);

  // Focus edit input when editing
  useEffect(() => {
    if (editing && editInputRef.current) {
      editInputRef.current.focus();
      editInputRef.current.setSelectionRange(editText.length, editText.length);
    }
  }, [editing]);

  // Extract external URLs from the text portion only (not uploaded media)
  const externalUrls = extractUrls(text).filter((u) => !isUploadUrl(u));

  // Show server username if available, then persona display name for own messages, then truncated pseudonym.
  let displayName: string;
  const cachedName = getDisplayName(message.sender_pseudonym);
  if (cachedName) {
    displayName = cachedName;
  } else if (isSelf && selfPersona?.displayName) {
    displayName = selfPersona.displayName;
  } else {
    displayName = message.sender_pseudonym.slice(0, 12) + '...';
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

  const handleDelete = useCallback(() => {
    deleteMessage(message.message_id);
  }, [message.message_id, deleteMessage]);

  const handleShowHistory = useCallback(async () => {
    if (showHistory) {
      setShowHistory(false);
      return;
    }
    setShowHistory(true);
    if (!editHistory && activeChannelId) {
      setHistoryLoading(true);
      try {
        const edits = await api.getMessageEdits(pseudonymId, activeChannelId, message.message_id);
        setEditHistory(edits);
      } catch {
        setEditHistory([]);
      } finally {
        setHistoryLoading(false);
      }
    }
  }, [showHistory, editHistory, pseudonymId, activeChannelId, message.message_id]);

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
    <div className={`message ${isSelf ? 'self' : ''} ${isDeleted ? 'deleted' : ''}`}>
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
            <button className="msg-action-btn edit-btn" onClick={handleEdit} title="Edit message">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" />
                <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" />
              </svg>
            </button>
            <button className="msg-action-btn delete-btn" onClick={handleDelete} title="Delete message">
              <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                <polyline points="3 6 5 6 21 6" />
                <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6m3 0V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
              </svg>
            </button>
          </span>
        )}
      </div>

      {isDeleted ? (
        <div className="message-content message-deleted-text">This message was deleted</div>
      ) : editing ? (
        <div className="message-edit-form">
          <textarea
            ref={editInputRef}
            className="message-edit-input"
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
                  src={url}
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
                  src={url}
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
                  href={url}
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
          ) : editHistory && editHistory.length > 0 ? (
            <div className="edit-history-list">
              {editHistory.map((edit) => (
                <div key={edit.id} className="edit-history-item">
                  <div className="edit-history-content">{edit.old_content}</div>
                  <div className="edit-history-time">
                    {new Date(edit.edited_at + 'Z').toLocaleString()}
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
  const { messages, activeChannelId, loadOlderMessages, loadingOlder, hasMoreMessages } = useChannelsStore();
  const loadVisibleUsernames = useUsernameStore((s) => s.loadVisibleUsernames);
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

  // Load visible usernames from server
  useEffect(() => {
    if (!identity?.pseudonymId) return;
    loadVisibleUsernames(identity.pseudonymId);
  }, [identity?.pseudonymId, loadVisibleUsernames]);

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
    if (el.scrollTop === 0 && messageCount > 0 && !loadingOlder && hasMoreMessages) {
      // Save scroll height so the effect can restore position after prepend
      prevScrollHeight.current = el.scrollHeight;
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
