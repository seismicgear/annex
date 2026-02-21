/**
 * Message input component — text input with send button and image upload.
 *
 * Supports image uploads via a paperclip/image button. Uploaded images
 * have their metadata (EXIF, GPS, etc.) stripped server-side for privacy.
 */

import { useState, useRef, type FormEvent, type KeyboardEvent } from 'react';
import { useChannelsStore } from '@/stores/channels';
import { useIdentityStore } from '@/stores/identity';
import * as api from '@/lib/api';

export function MessageInput() {
  const [content, setContent] = useState('');
  const [uploading, setUploading] = useState(false);
  const [preview, setPreview] = useState<{ file: File; dataUrl: string } | null>(null);
  const fileInputRef = useRef<HTMLInputElement>(null);
  const { activeChannelId, wsConnected, sendMessage } = useChannelsStore();
  const identity = useIdentityStore((s) => s.identity);

  const handleSubmit = async (e: FormEvent) => {
    e.preventDefault();
    if (!activeChannelId || !identity?.pseudonymId) return;

    // If there's a pending image, upload it first
    if (preview) {
      setUploading(true);
      try {
        const resp = await api.uploadChatImage(
          identity.pseudonymId,
          activeChannelId,
          preview.file,
        );
        // Send message with image URL (with optional text)
        const text = content.trim();
        const msgContent = text
          ? `${text}\n${resp.url}`
          : resp.url;
        sendMessage(msgContent);
        setContent('');
        setPreview(null);
      } catch (err) {
        console.error('Image upload failed:', err);
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

    // Validate file type
    const allowed = ['image/jpeg', 'image/png', 'image/gif', 'image/webp'];
    if (!allowed.includes(file.type)) {
      return;
    }

    // Validate file size (10 MiB max)
    if (file.size > 10 * 1024 * 1024) {
      return;
    }

    // Create preview
    const reader = new FileReader();
    reader.onload = () => {
      setPreview({ file, dataUrl: reader.result as string });
    };
    reader.readAsDataURL(file);

    // Reset input so the same file can be re-selected
    e.target.value = '';
  };

  const cancelPreview = () => {
    setPreview(null);
  };

  if (!activeChannelId) return null;

  return (
    <div className="message-input-wrapper">
      {preview && (
        <div className="image-preview-bar">
          <img src={preview.dataUrl} alt="Preview" className="image-preview-thumb" />
          <span className="image-preview-name">{preview.file.name}</span>
          <button
            className="image-preview-cancel"
            onClick={cancelPreview}
            title="Remove image"
          >
            x
          </button>
        </div>
      )}
      <form className="message-input" onSubmit={handleSubmit}>
        <input
          ref={fileInputRef}
          type="file"
          accept="image/jpeg,image/png,image/gif,image/webp"
          onChange={handleFileSelect}
          style={{ display: 'none' }}
        />
        <button
          type="button"
          className="image-upload-btn"
          onClick={() => fileInputRef.current?.click()}
          disabled={!wsConnected || uploading}
          title="Upload image (metadata will be stripped for privacy)"
        >
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2">
            <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
            <circle cx="8.5" cy="8.5" r="1.5" />
            <polyline points="21 15 16 10 5 21" />
          </svg>
        </button>
        <textarea
          value={content}
          onChange={(e) => setContent(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder={
            uploading
              ? 'Uploading image...'
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
