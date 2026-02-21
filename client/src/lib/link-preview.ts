/**
 * Privacy-preserving link preview — fetches OpenGraph metadata through
 * a server-side anonymizing proxy to avoid leaking user IPs.
 *
 * URL detection is done client-side; the actual HTTP fetch goes through
 * the server's /api/link-preview endpoint which:
 * - Fetches on behalf of all users (one fetch per unique URL)
 * - Caches results to avoid repeated external requests
 * - Strips any user-identifying headers
 *
 * If the proxy endpoint is unavailable, previews gracefully degrade
 * to showing just the URL as a clickable link.
 */

import type { LinkPreviewData } from '@/types';
import { getApiBaseUrl } from '@/lib/api';

// Match URLs in message text
const URL_REGEX = /https?:\/\/[^\s<>'")\]]+/gi;

// In-memory cache to avoid duplicate requests per session.
// Bounded to prevent unbounded growth during long sessions.
const MAX_CACHE_SIZE = 500;
const previewCache = new Map<string, LinkPreviewData>();

/** Extract all URLs from a message string. */
export function extractUrls(text: string): string[] {
  const matches = text.match(URL_REGEX);
  if (!matches) return [];
  // Deduplicate
  return [...new Set(matches)];
}

/**
 * Fetch link preview data for a URL through the server-side proxy.
 * Returns cached data if available.
 */
export async function fetchLinkPreview(
  url: string,
  pseudonymId: string,
): Promise<LinkPreviewData> {
  const cached = previewCache.get(url);
  if (cached && !cached.loading) return cached;

  // Evict oldest entry before inserting to keep cache within bounds
  if (previewCache.size >= MAX_CACHE_SIZE) {
    const firstKey = previewCache.keys().next().value;
    if (firstKey !== undefined) previewCache.delete(firstKey);
  }

  const loading: LinkPreviewData = {
    url,
    title: null,
    description: null,
    imageUrl: null,
    siteName: null,
    loading: true,
    error: null,
  };
  previewCache.set(url, loading);

  try {
    // Use API base URL so link previews route to the active server
    const base = getApiBaseUrl();
    const endpoint = `${base}/api/link-preview?` + new URLSearchParams({ url });
    const res = await fetch(endpoint, {
      headers: { 'X-Annex-Pseudonym': pseudonymId },
    });

    if (!res.ok) {
      // Server proxy not available — degrade gracefully
      const fallback: LinkPreviewData = {
        url,
        title: null,
        description: null,
        imageUrl: null,
        siteName: extractDomain(url),
        loading: false,
        error: res.status === 404 ? 'Preview proxy not configured' : `HTTP ${res.status}`,
      };
      previewCache.set(url, fallback);
      return fallback;
    }

    const data = await res.json();
    // Route OG images through the server-side proxy so the user's IP
    // is never exposed to the third-party image host.
    const rawImageUrl: string | null = data.image_url ?? null;
    const proxiedImageUrl = rawImageUrl
      ? `${base}/api/link-preview/image?` + new URLSearchParams({ url: rawImageUrl })
      : null;
    const preview: LinkPreviewData = {
      url,
      title: data.title ?? null,
      description: data.description ?? null,
      imageUrl: proxiedImageUrl,
      siteName: data.site_name ?? extractDomain(url),
      loading: false,
      error: null,
    };
    previewCache.set(url, preview);
    return preview;
  } catch {
    const fallback: LinkPreviewData = {
      url,
      title: null,
      description: null,
      imageUrl: null,
      siteName: extractDomain(url),
      loading: false,
      error: 'Network error',
    };
    previewCache.set(url, fallback);
    return fallback;
  }
}

/** Extract domain from a URL for display as site name. */
function extractDomain(url: string): string {
  try {
    return new URL(url).hostname;
  } catch {
    return url;
  }
}

/** Clear the preview cache (useful on identity switch). */
export function clearPreviewCache(): void {
  previewCache.clear();
}
