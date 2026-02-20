/**
 * Link preview card — renders rich OpenGraph previews for URLs in messages.
 *
 * Fetches preview data through the server-side anonymizing proxy so that
 * individual user IPs are never exposed to third-party sites.
 * Degrades gracefully if the proxy is unavailable.
 */

import { useState, useEffect } from 'react';
import type { LinkPreviewData } from '@/types';
import { fetchLinkPreview } from '@/lib/link-preview';

interface Props {
  url: string;
  pseudonymId: string;
}

export function LinkPreview({ url, pseudonymId }: Props) {
  const [preview, setPreview] = useState<LinkPreviewData | null>(null);

  useEffect(() => {
    let cancelled = false;

    fetchLinkPreview(url, pseudonymId).then((data) => {
      if (!cancelled) setPreview(data);
    });

    return () => {
      cancelled = true;
    };
  }, [url, pseudonymId]);

  if (!preview || preview.loading) {
    return (
      <div className="link-preview loading">
        <div className="link-preview-shimmer" />
      </div>
    );
  }

  // If proxy failed, show minimal clickable link
  if (preview.error && !preview.title) {
    return (
      <a
        href={url}
        target="_blank"
        rel="noopener noreferrer"
        className="link-preview-minimal"
      >
        <span className="link-preview-domain">{preview.siteName}</span>
        <span className="link-preview-url">{url}</span>
      </a>
    );
  }

  return (
    <a
      href={url}
      target="_blank"
      rel="noopener noreferrer"
      className="link-preview-card"
    >
      {preview.imageUrl && (
        <div className="link-preview-image">
          <img
            src={preview.imageUrl}
            alt=""
            loading="lazy"
            onError={(e) => {
              (e.target as HTMLImageElement).style.display = 'none';
            }}
          />
        </div>
      )}
      <div className="link-preview-body">
        {preview.siteName && (
          <span className="link-preview-site">{preview.siteName}</span>
        )}
        {preview.title && (
          <span className="link-preview-title">{preview.title}</span>
        )}
        {preview.description && (
          <span className="link-preview-desc">
            {preview.description.length > 150
              ? preview.description.slice(0, 150) + '...'
              : preview.description}
          </span>
        )}
      </div>
    </a>
  );
}
