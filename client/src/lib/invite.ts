/**
 * Invite routing — parses invite links and orchestrates background
 * identity verification + server join.
 *
 * New invite URL format (monolithannex.com):
 *   https://monolithannex.com/invite/{base64url_payload}
 *   → deep-links to annex://invite?server={encoded}&code={encoded}
 *
 * The `annex://` half is parsed in `annex-desktop`, not here — the OS hands
 * the URL to the Tauri process, which validates it and emits the result as
 * an event. This module owns the http(s) links the browser sees.
 *
 * Legacy invite URL format (direct server links):
 *   https://<host>/invite/<channelId>?slug=<serverSlug>&label=<label>
 */

import type { LegacyInvitePayload } from '@/types';

/**
 * Parse an invite from the current window location.
 *
 * Supports legacy channel-based invite links for backwards compatibility.
 * Returns null if the current URL is not an invite link.
 */
export function parseLegacyInviteFromUrl(): LegacyInvitePayload | null {
  const url = new URL(window.location.href);
  const pathParts = url.pathname.split('/').filter(Boolean);

  // Match /invite/<channelId>
  if (pathParts.length >= 2 && pathParts[0] === 'invite') {
    const channelId = pathParts[1];
    const serverSlug = url.searchParams.get('slug') ?? 'default';
    const label = url.searchParams.get('label') ?? undefined;

    return {
      server: url.host,
      channelId,
      serverSlug,
      label,
    };
  }

  // Also check for hash-based invites: #/invite/<channelId>
  if (url.hash.startsWith('#/invite/')) {
    const hashParts = url.hash.slice(2).split('/').filter(Boolean);
    if (hashParts.length >= 2) {
      const channelId = hashParts[1];
      const hashParams = new URLSearchParams(url.hash.split('?')[1] ?? '');
      return {
        server: url.host,
        channelId,
        serverSlug: hashParams.get('slug') ?? 'default',
        label: hashParams.get('label') ?? undefined,
      };
    }
  }

  return null;
}

/**
 * Create a new invite by calling the server API.
 *
 * Uses the shared auth plumbing from api.ts so the Bearer token and any
 * required proof headers are sent consistently (required for secure desktop
 * builds where X-Annex-Pseudonym alone is insufficient).
 *
 * Returns the full monolithannex.com shareable URL.
 */
/**
 * Can an invite link be built from this server's public URL?
 *
 * The invite format requires HTTPS — `InvitePayload::validate` on the server
 * rejects anything else, because the link carries a join secret that must not
 * be readable in transit. Callers used to test only that a public URL was
 * *set*, so every http:// deployment fired an invite request that could never
 * succeed: the admin panel did it twice (on open, and after saving a URL) and
 * startup did it once more, each swallowing the failure in an empty catch. The
 * operator saw no invite link and no reason for its absence, while the server
 * log filled with rejected requests. The UI audit caught it as a repeated 400
 * on `admin-server-settings`.
 */
export function canCreateInviteLink(publicUrl: string | null | undefined): boolean {
  return /^https:\/\//i.test((publicUrl ?? '').trim());
}

export async function createInviteLink(
  _apiBaseUrl: string,
  pseudonymId: string,
  options?: { maxUses?: number; expiresInHours?: number },
): Promise<{ code: string; url: string; expiresAt?: string }> {
  // Dynamically import to avoid circular dependency at module level
  const api = await import('@/lib/api');
  return api.createInvite(pseudonymId, options);
}

/** Clear the invite state from the URL without a page reload. */
export function clearInviteFromUrl(): void {
  const url = new URL(window.location.href);
  if (url.pathname.startsWith('/invite/') || url.hash.startsWith('#/invite/')) {
    window.history.replaceState(null, '', '/');
  }
}

export interface InviteProgress {
  stage: 'parsing' | 'registering' | 'proving' | 'joining' | 'complete' | 'error';
  message: string;
}

export type InviteProgressCallback = (progress: InviteProgress) => void;
