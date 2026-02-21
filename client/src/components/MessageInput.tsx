/**
 * Message input component — text input with send button and media/file upload.
 *
 * Supports image, video, and file uploads via an attachment button.
 * Uploaded images have their metadata (EXIF, GPS, etc.) stripped server-side
 * for privacy. Videos and files are MIME-verified server-side.
 */

import { useState, useRef, type FormEvent, type KeyboardEvent } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import * as api from '@/lib/api';

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
  const [content, setContent] = useState('');
  const [uploading, setUploading] = useState(false);
  const [preview, setPreview] = useState<FilePreview | null>(null);
  const [uploadError, setUploadError] = useState<string | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const { activeChannelId, wsConnected, sendMessage } = useChannelsStore();
  const identity = useIdentityStore((s) => s.identity);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!activeChannelId || !identity?.pseudonymId) return;
    setUploadError(null);

    // If there's a pending file, upload it first
    if (preview) {
      setUploading(true);
      try {
        const resp = await api.uploadChatFile(
          identity.pseudonymId,
          activeChannelId,
          preview.file,
        );
        // Send message with file URL (with optional text)
        const text = content.trim();
        const msgContent = text
          ? `${text}\n${resp.url}`
          : resp.url;
        sendMessage(msgContent);
        setContent('');
        setPreview(null);
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err);
        setUploadError(`Upload failed: ${msg}`);
        console.error('Upload failed:', err);
      } finally {
        setUploading(false);
      }
      return;
    }

    // Regular text message
    const trimmed = content.trim();
    if (!trimmed) return;
    sendMessage(trimmed);
    setContent('');
  };

  const handleKeyDown = (e: KeyboardEvent) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      handleSubmit(e);
    }
  };

  const handleFileSelect = (e: React.ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    if (!file) return;
    setUploadError(null);

    const category = fileCategoryLabel(file);

    // Create preview for images; for videos/files just show metadata
    if (isPreviewableImage(file)) {
      const reader = new FileReader();
      reader.onload = () => {
        setPreview({ file, dataUrl: reader.result as string, category });
      };
      reader.readAsDataURL(file);
    } else if (isVideo(file)) {
      // Create video thumbnail via object URL
      const objectUrl = URL.createObjectURL(file);
      setPreview({ file, dataUrl: objectUrl, category });
    } else {
      setPreview({ file, dataUrl: null, category });
    }

    // Reset input so the same file can be re-selected
    e.target.value = '';
  };

  const cancelPreview = () => {
    if (preview?.dataUrl && isVideo(preview.file)) {
      URL.revokeObjectURL(preview.dataUrl);
    }
    setPreview(null);
    setUploadError(null);
  };

  if (!activeChannelId) return null;

  return (
    <div className="message-input-wrapper">
      {uploadError && (
        <div className="upload-error-bar">{uploadError}</div>
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
          disabled={!wsConnected || uploading}
          title="Upload image, video, or file"
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <path d="M21.44 11.05l-9.19 9.19a6 6 0 0 1-8.49-8.49l9.19-9.19a4 4 0 0 1 5.66 5.66l-9.2 9.19a2 2 0 0 1-2.83-2.83l8.49-8.48" />
          </svg>
        </button>
        <textarea
          value={content}
          onChange={(e) => setContent(e.target.value)}
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
          disabled={!wsConnected || uploading || (!content.trim() && !preview)}
        >
          {uploading ? '...' : 'Send'}
        </button>
      </form>
    </div>
  );
}
