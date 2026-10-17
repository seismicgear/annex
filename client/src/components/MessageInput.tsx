/**
 * Message input component — text input with send button and media/file upload.
 *
 * Supports image, video, and file uploads via an attachment button.
 * Uploaded images have their metadata (EXIF, GPS, etc.) stripped server-side
 * for privacy. Videos and files are MIME-verified server-side.
 */

import { useState, useRef, useEffect, type FormEvent, type KeyboardEvent } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useUsernameStore } from '@/stores/usernames';
import { useIdentityStore } from '@/stores/identity';
import * as api from '@/lib/api';
import { getCredentialContext, isCredentialContextCurrent } from '@/api/core';
import { abortStagedUpload, uploadEncryptedAttachment } from '@/api/encrypted-uploads';
import { isChannelE2e, isChannelE2eUnknown } from '@/lib/message-crypto';
import { messageText, privateMessage } from '@/lib/encrypted-attachments';
import { UploadCleanupNotice } from './UploadCleanupNotice';

/** Allowed MIME types by upload category. */
const IMAGE_TYPES = ['image/jpeg', 'image/png', 'image/gif', 'image/webp'];
const VIDEO_TYPES = ['video/mp4', 'video/webm', 'video/quicktime'];

/** Returns a human-readable file category label. */
function fileCategoryLabel(file: File): string {
  if (IMAGE_TYPES.includes(file.type)) return 'image';
  if (VIDEO_TYPES.includes(file.type)) return 'video';
  return 'file';
}

/** Returns true if the file is a previewable image. */
function isPreviewableImage(file: File): boolean {
  return IMAGE_TYPES.includes(file.type);
}

/** Returns true if the file is a video. */
function isVideo(file: File): boolean {
  return VIDEO_TYPES.includes(file.type);
}

/** Format file size for display. */
function formatSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

interface FilePreview {
  file: File;
  dataUrl: string | null;
  category: string;
}

export function MessageInput() {
  const channel = useChannelsStore(state => state.activeChannelId);
  const identity = useIdentityStore(state => state.identity?.pseudonymId);
  return <><UploadCleanupNotice /><MessageComposer key={JSON.stringify([getCredentialContext().serial, channel, identity])} /></>;
}

function MessageComposer() {
  const [content, setContent] = useState('');
  const [uploading, setUploading] = useState(false);
  const [preview, setPreview] = useState<FilePreview | null>(null);
  const [dragOver, setDragOver] = useState(false);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const [sendFailure, setSendFailure] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const { activeChannelId, activeChannelKeyState, wsConnected, sendMessage, composerError, clearComposerError, sendTyping, replyToMessage, setReplyTo } = useChannelsStore();
  const channelReady = activeChannelKeyState === 'ready';
  const identity = useIdentityStore((s) => s.identity);
  const pseudonymId = identity?.pseudonymId ?? '';

  // The reply bar showed `sender_pseudonym.slice(0, 12) + '...'` unconditionally
  // — a hex string, even when the server username for that person was already
  // resolved and rendered as a name on the very message being replied to. The
  // message bubble two lines above it got this right; the composer did not.
  const getDisplayName = useUsernameStore((s) => s.getDisplayName);
  const replyAuthorPseudonym = replyToMessage?.sender_pseudonym ?? '';
  const replyAuthorResolved = replyAuthorPseudonym ? getDisplayName(replyAuthorPseudonym) : undefined;
  const replyAuthorIsPseudonym = !replyAuthorResolved;
  const replyAuthorName = replyAuthorResolved ?? `${replyAuthorPseudonym.slice(0, 12)}...`;

  // Track the pending request ID so we know when the server acknowledges it.
  const pendingRequestIdRef = useRef<string | null>(null);
  const activeUpload = useRef<AbortController | null>(null);
  const selectionGeneration = useRef(0);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!wsConnected || uploading || !channelReady || !activeChannelId || !identity?.pseudonymId) return;
    setUploadError(null);
    setSendFailure(null);

    // If there's a pending file, upload it first
    if (preview) {
      setUploading(true);
      const captured = getCredentialContext();
      const abort = new AbortController(); activeUpload.current = abort;
      let staged: string | null = null;
      try {
        if (isChannelE2eUnknown(activeChannelId)) throw new Error('Verify channel encryption before uploading.');
        const text = content.trim();
        let msgContent: string;
        if (isChannelE2e(activeChannelId)) {
          const descriptor = await uploadEncryptedAttachment(pseudonymId, activeChannelId, preview.file, abort.signal);
          staged = descriptor.id; msgContent = privateMessage(text, [descriptor]);
        } else {
          const resp = await api.uploadChatFile(pseudonymId, activeChannelId, preview.file, abort.signal);
          if (!resp.upload_id) throw new Error('The server did not return an attachment ID.');
          staged = resp.upload_id; msgContent = text ? `${text}\n${resp.url}` : resp.url;
        }
        abort.signal.throwIfAborted();
        if (!isCredentialContextCurrent(captured)) throw new Error('The attachment belongs to a previous session.');
        const reqId = sendMessage(msgContent, pseudonymId, undefined, [staged]);
        if (reqId) {
          staged = null; // The optimistic message now owns abort/retry cleanup.
          pendingRequestIdRef.current = reqId;
          setContent('');
          if (preview.dataUrl && isVideo(preview.file)) URL.revokeObjectURL(preview.dataUrl);
          setPreview(null);
        }
      } catch (err) {
        if (!abort.signal.aborted) setUploadError(`Upload failed: ${err instanceof Error ? err.message : 'The upload was not confirmed.'}`);
      } finally {
        if (staged && isCredentialContextCurrent(captured)) {
          try { await abortStagedUpload(pseudonymId, activeChannelId, staged); }
          catch (err) { if (!abort.signal.aborted) setUploadError(`Attachment cleanup failed: ${err instanceof Error ? err.message : 'Retry removing the attachment.'}`); }
        }
        if (activeUpload.current === abort) activeUpload.current = null;
        if (!abort.signal.aborted) setUploading(false);
      }
      return;
    }

    // Regular text message
    const trimmed = content.trim();
    if (!trimmed) return;
    const reqId = sendMessage(trimmed, pseudonymId);
    if (reqId) {
      pendingRequestIdRef.current = reqId;
      setContent('');
    }
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSubmit(e);
    }
  };

  const processFile = (file: File) => {
    if (!wsConnected || uploading) return;
    const generation = ++selectionGeneration.current;
    if (preview?.dataUrl && isVideo(preview.file)) URL.revokeObjectURL(preview.dataUrl);
    setUploadError(null);
    const category = fileCategoryLabel(file);

    if (isPreviewableImage(file)) {
      const reader = new FileReader();
      reader.onload = () => {
        if (generation === selectionGeneration.current) setPreview({ file, dataUrl: reader.result as string, category });
      };
      reader.onerror = () => { if (generation === selectionGeneration.current) setUploadError('The selected file could not be read.'); };
      reader.readAsDataURL(file);
    } else if (isVideo(file)) {
      const objectUrl = URL.createObjectURL(file);
      setPreview({ file, dataUrl: objectUrl, category });
    } else {
      setPreview({ file, dataUrl: null, category });
    }
  };

  const handleFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    processFile(file);

    // Reset input so the same file can be re-selected
    e.target.value = '';
  };

  const cancelPreview = () => {
    selectionGeneration.current++;
    activeUpload.current?.abort();
    setUploading(false);
    if (preview?.dataUrl && isVideo(preview.file)) {
      URL.revokeObjectURL(preview.dataUrl);
    }
    setPreview(null);
    setUploadError(null);
  };

  // Track latest preview in a ref so the unmount cleanup can access
  // the current value rather than the stale initial render closure.
  const previewRef = useRef(preview);
  previewRef.current = preview;

  // Revoke video object URLs on unmount to prevent memory leaks
  useEffect(() => {
    const generation = selectionGeneration;
    return () => {
      generation.current++;
      activeUpload.current?.abort();
      const p = previewRef.current;
      if (p?.dataUrl && isVideo(p.file)) {
        URL.revokeObjectURL(p.dataUrl);
      }
    };
  }, []);

  const handleDrop = (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);
    const file = e.dataTransfer.files[0];
    if (file) processFile(file);
  };

  const handleDragOver = (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(true);
  };

  const handleDragLeave = (e: React.DragEvent) => {
    e.preventDefault();
    setDragOver(false);
  };

  if (!activeChannelId) return null;

  return (
    <div
      className={`message-input-wrapper ${dragOver ? 'drag-over' : ''}`}
      onDrop={handleDrop}
      onDragOver={handleDragOver}
      onDragLeave={handleDragLeave}
    >
      {(composerError || sendFailure) && (
        <div className="upload-error-bar composer-error" role="alert">
          <span>{sendFailure ?? composerError}</span>
          <button
            onClick={() => { clearComposerError(); setSendFailure(null); }}
            className="composer-error-dismiss"
            aria-label="Dismiss"
          >
            &times;
          </button>
        </div>
      )}
      {replyToMessage && (
        <div className="reply-bar">
          <span className="reply-bar-label">Replying to</span>
          <span className={replyAuthorIsPseudonym ? 'reply-bar-author reply-bar-author-pseudonym' : 'reply-bar-author'}>
            {replyAuthorName}
          </span>
          <span className="reply-bar-text">{messageText(replyToMessage.content).slice(0, 80)}{messageText(replyToMessage.content).length > 80 ? '...' : ''}</span>
          <button className="reply-bar-cancel" onClick={() => setReplyTo(null)} aria-label="Cancel reply">&times;</button>
        </div>
      )}
      {uploadError && (
        // `role="alert"`, like every other error surface in the app. Without
        // it this bar appears silently: a screen-reader user presses Send,
        // nothing is announced, and the composer still holds the attachment
        // with no indication the upload was refused.
        <div className="upload-error-bar" role="alert">{uploadError}</div>
      )}
      {preview && (
        <div className="image-preview-bar">
          {preview.category === 'image' && preview.dataUrl && (
            <img src={preview.dataUrl} alt="Preview" className="image-preview-thumb" />
          )}
          {preview.category === 'video' && preview.dataUrl && (
            <video
              src={preview.dataUrl}
              className="image-preview-thumb"
              muted
              playsInline
            />
          )}
          {preview.category === 'file' && (
            <span className="file-preview-icon">
              <svg width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
                <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
                <polyline points="14 2 14 8 20 8" />
              </svg>
            </span>
          )}
          <span className="image-preview-name">
            {preview.file.name}
            <span className="image-preview-meta"> ({formatSize(preview.file.size)}, {preview.category})</span>
          </span>
          <button
            className="image-preview-cancel"
            onClick={cancelPreview}
            title="Remove attachment"
          >
            x
          </button>
        </div>
      )}
      <form className="message-input" onSubmit={handleSubmit}>
        <input
          ref={fileInputRef}
          type="file"
          accept="image/jpeg,image/png,image/gif,image/webp,video/mp4,video/webm,video/quicktime,application/pdf,application/zip,text/plain"
          onChange={handleFileSelect}
          style={{ display: 'none' }}
        />
        <button
          type="button"
          className="image-upload-btn"
          onClick={() => fileInputRef.current?.click()}
          disabled={!wsConnected || uploading || !channelReady}
          title="Upload image, video, or file"
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
          </svg>
        </button>
        <textarea
          value={content}
          onChange={(e) => { setContent(e.target.value); sendTyping(); }}
          onKeyDown={handleKeyDown}
          placeholder={
            uploading
              ? 'Uploading...'
              : preview
                ? 'Add a caption (optional)...'
                : wsConnected
                  ? 'Type a message...'
                  : 'Connecting...'
          }
          disabled={!wsConnected || uploading}
          rows={1}
        />
        <button
          type="submit"
          disabled={!wsConnected || uploading || !channelReady || (!content.trim() && !preview)}
        >
          {uploading ? '...' : 'Send'}
        </button>
      </form>
    </div>
  );
}
